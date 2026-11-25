//! Keeping everyone's copy of the map the same while several people edit it.
//!
//! Nobody sends the whole map for every change. Each editor remembers the map
//! as it last agreed with the host (its "base") and, a few times a second,
//! sends the difference between that and what's on its screen as [`Op`]s.
//! Pieces have no ids, so an op names a piece by its value (and where it
//! was): "the crate at 3, 0, 4 is now at 5, 0, 4". That still finds the right
//! piece when someone else added or removed pieces before it meanwhile.
//! Everything that isn't a piece (name, sky, size, spawns, ...) goes field
//! by field, so two people changing different settings don't undo each other.
//!
//! The host's map is the real one. Every message from the host carries a hash
//! of its map; when an editor's base hash differs (two people moved the same
//! piece at once, say), it asks for the whole map again.

use ink_map::{MapFile, Object};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// More pieces changed than this at once: send the whole map instead.
const MAX_OPS: usize = 48;

/// One change to the map.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum Op {
    /// The piece `old` (at index `at`, or wherever it is now) became `new`.
    Change { at: usize, old: Object, new: Object },
    /// The piece `old` (at `at`, or wherever it is now) is gone.
    Remove { at: usize, old: Object },
    /// A new piece, at index `at` (or the end).
    Add { at: usize, obj: Object },
    /// These map settings changed (top-level fields of the map file, without
    /// `objects`): name, sky, size, ground, spawns and so on.
    Fields(Map<String, Value>),
    /// The whole map (New or Open on the host, or after getting out of step).
    Full(Box<MapFile>),
}

/// What an op did when applied, for the scene and the selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    /// Pieces were added or removed, or the field changed: rebuild the scene.
    pub structure: bool,
    /// The op couldn't be applied (the piece wasn't there).
    pub missed: bool,
    /// A piece was removed at this index (selections after it shift down).
    pub removed: Option<usize>,
    /// A piece was inserted at this index (selections at or after it shift up).
    pub inserted: Option<usize>,
}

/// Settings that need the scene rebuilt when they change.
const STRUCTURAL: [&str; 3] = ["half", "ground", "perimeter"];

/// The map's settings as JSON fields (everything but the pieces).
fn fields(map: &MapFile) -> Map<String, Value> {
    match serde_json::to_value(map) {
        Ok(Value::Object(mut m)) => {
            m.remove("objects");
            m
        }
        _ => Map::new(),
    }
}

/// What turns `base` into `now`. Empty when they're the same.
pub fn diff(base: &MapFile, now: &MapFile) -> Vec<Op> {
    let mut ops = Vec::new();

    let (a, b) = (&base.objects, &now.objects);
    if a.len() == b.len() {
        for (i, (old, new)) in a.iter().zip(b).enumerate() {
            if old != new {
                ops.push(Op::Change { at: i, old: old.clone(), new: new.clone() });
            }
        }
    } else {
        // What's the same at the start and the end; the middle changed.
        let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
        let room = a.len().min(b.len()) - prefix;
        let suffix = a.iter().rev().zip(b.iter().rev()).take(room).take_while(|(x, y)| x == y).count();
        let removed = &a[prefix..a.len() - suffix];
        let added = &b[prefix..b.len() - suffix];
        // Back to front, so each index is still right when it's applied.
        for (k, old) in removed.iter().enumerate().rev() {
            ops.push(Op::Remove { at: prefix + k, old: old.clone() });
        }
        for (k, obj) in added.iter().enumerate() {
            ops.push(Op::Add { at: prefix + k, obj: obj.clone() });
        }
    }
    if ops.len() > MAX_OPS {
        return vec![Op::Full(Box::new(now.clone()))];
    }

    let (fa, fb) = (fields(base), fields(now));
    let changed: Map<String, Value> = fb
        .iter()
        .filter(|(k, v)| fa.get(*k) != Some(*v))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if !changed.is_empty() {
        ops.push(Op::Fields(changed));
    }
    ops
}

/// Where `want` is in `list`: at `hint` if it's there, else the nearest copy.
fn find(list: &[Object], hint: usize, want: &Object) -> Option<usize> {
    if list.get(hint) == Some(want) {
        return Some(hint);
    }
    list.iter()
        .enumerate()
        .filter(|(_, o)| *o == want)
        .min_by_key(|(i, _)| i.abs_diff(hint))
        .map(|(i, _)| i)
}

/// Applies one op to `map`.
pub fn apply(map: &mut MapFile, op: &Op) -> Applied {
    let mut out = Applied::default();
    match op {
        Op::Change { at, old, new } => match find(&map.objects, *at, old) {
            Some(i) => {
                if map.objects[i].piece != new.piece {
                    out.structure = true;
                }
                map.objects[i] = new.clone();
            }
            None => out.missed = true,
        },
        Op::Remove { at, old } => match find(&map.objects, *at, old) {
            Some(i) => {
                map.objects.remove(i);
                out.structure = true;
                out.removed = Some(i);
            }
            None => out.missed = true,
        },
        Op::Add { at, obj } => {
            let i = (*at).min(map.objects.len());
            map.objects.insert(i, obj.clone());
            out.structure = true;
            out.inserted = Some(i);
        }
        Op::Fields(changed) => {
            let mut now = fields(map);
            for (k, v) in changed {
                if STRUCTURAL.contains(&k.as_str()) && now.get(k) != Some(v) {
                    out.structure = true;
                }
                now.insert(k.clone(), v.clone());
            }
            now.insert("objects".into(), Value::Array(Vec::new()));
            match serde_json::from_value::<MapFile>(Value::Object(now)) {
                Ok(mut next) => {
                    next.objects = std::mem::take(&mut map.objects);
                    *map = next;
                }
                Err(_) => out.missed = true,
            }
        }
        Op::Full(next) => {
            *map = (**next).clone();
            out.structure = true;
        }
    }
    out
}

/// A hash of the whole map, the same on every PC for the same map.
pub fn hash(map: &MapFile) -> u64 {
    // FNV-1a over the JSON (std's hasher isn't promised to match between builds).
    let bytes = serde_json::to_vec(map).unwrap_or_default();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(piece: &str, x: f32) -> Object {
        Object::new(piece, [x, 0.0, 0.0], 0.0)
    }

    fn map_with(objects: Vec<Object>) -> MapFile {
        let mut m = MapFile::blank("Test");
        m.objects = objects;
        m
    }

    /// Whatever changed, applying the diff to the base gives the new map.
    #[test]
    fn diff_then_apply_gives_the_same_map() {
        let base = map_with(vec![obj("crate", 1.0), obj("can", 2.0), obj("snake", 3.0)]);
        let cases = [
            map_with(vec![obj("crate", 1.0), obj("can", 9.0), obj("snake", 3.0)]),
            map_with(vec![obj("crate", 1.0), obj("can", 2.0), obj("snake", 3.0), obj("tree", 4.0)]),
            map_with(vec![obj("crate", 1.0), obj("snake", 3.0)]),
            map_with(vec![obj("tree", 0.0), obj("crate", 1.0), obj("can", 2.0), obj("snake", 3.0)]),
            map_with(vec![]),
            {
                let mut m = base.clone();
                m.name = "Renamed".into();
                m.sky = [0.1, 0.2, 0.3];
                m.spawns.blue.push([1.0, 2.0]);
                m
            },
        ];
        for now in cases {
            let mut m = base.clone();
            for op in diff(&base, &now) {
                assert!(!apply(&mut m, &op).missed, "{op:?}");
            }
            assert_eq!(m, now);
            assert_eq!(hash(&m), hash(&now));
        }
        assert!(diff(&base, &base).is_empty());
    }

    /// Two people editing at once: a move still finds its piece after someone
    /// else removed one before it, and settings merge field by field.
    #[test]
    fn concurrent_edits_merge() {
        let base = map_with(vec![obj("crate", 1.0), obj("can", 2.0), obj("snake", 3.0)]);
        let mut mine = base.clone();
        mine.objects[2].pos[0] = 30.0;
        mine.name = "Mine".into();
        let mut theirs = base.clone();
        theirs.objects.remove(0);
        theirs.sky = [0.0, 0.0, 0.0];

        // The host has their change; mine arrives.
        let mut host = theirs.clone();
        for op in diff(&base, &mine) {
            assert!(!apply(&mut host, &op).missed, "{op:?}");
        }
        assert_eq!(host.objects.len(), 2);
        assert_eq!(host.objects[1].pos[0], 30.0);
        assert_eq!(host.name, "Mine");
        assert_eq!(host.sky, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn a_change_to_a_piece_thats_gone_is_a_miss() {
        let mut m = map_with(vec![obj("crate", 1.0)]);
        let op = Op::Change { at: 0, old: obj("can", 5.0), new: obj("can", 6.0) };
        assert!(apply(&mut m, &op).missed);
        assert_eq!(m.objects, vec![obj("crate", 1.0)]);
    }

    #[test]
    fn many_changes_send_the_whole_map() {
        let base = map_with(vec![]);
        let now = map_with((0..100).map(|i| obj("crate", i as f32)).collect());
        let ops = diff(&base, &now);
        assert!(matches!(ops.as_slice(), [Op::Full(_)]));
        let mut m = base.clone();
        apply(&mut m, &ops[0]);
        assert_eq!(m, now);
    }

    #[test]
    fn ops_survive_json() {
        let base = map_with(vec![obj("crate", 1.0)]);
        let mut now = base.clone();
        now.objects[0].pos = [0.1, 0.2, 0.3];
        now.objects[0].yaw = 33.3;
        now.half = [21.5, 30.0];
        let ops: Vec<Op> = serde_json::from_slice(&serde_json::to_vec(&diff(&base, &now)).unwrap()).unwrap();
        let mut m = base.clone();
        for op in &ops {
            apply(&mut m, op);
        }
        assert_eq!(m, now);
    }
}
