"""Every piece the map editor can place.

Map files name pieces by `id`, so ids must never change once maps use them.
`gen.py` reads this list, fits collision shapes to each model and writes
`crates/ink-map/src/pieces.rs`.

Each entry: (id, label, category, model under assets/, options). Options:
  scenery  - nothing collides with it (ground decor, grass, bushes).
  decals   - whether paint splats stick to it (default: yes unless its
             shapes are rough fits of an organic shape).
  limit    - most collision shapes to use (default 16).
  skip     - material name parts to leave out of the collision (water).
  parts    - hand-written shapes in Rust, used instead of fitting.
"""

import os

# Pieces that were in the game before; they keep their ids and labels.
EXISTING = [
    # Inflatable bunkers.
    ("can", "Can", "Bunkers", "models/bunkers/can.glb", {}),
    ("mini_can", "Mini can", "Bunkers", "models/bunkers/mini_can.glb", {}),
    ("cake", "Cake", "Bunkers", "models/bunkers/cake.glb", {}),
    ("bunker_tree", "Tree bunker", "Bunkers", "models/bunkers/tree.glb", {"decals": False}),
    ("brick", "Brick", "Bunkers", "models/bunkers/brick.glb", {}),
    ("dorito", "Dorito", "Bunkers", "models/bunkers/dorito.glb", {}),
    ("snake", "Snake", "Bunkers", "models/bunkers/snake.glb", {}),
    ("tall_wall", "Tall wall", "Bunkers", "models/bunkers/tall_wall.glb", {}),
    ("temple", "Temple", "Bunkers", "models/bunkers/temple.glb", {}),
    # Walls.
    ("wall_2m", "Wall 2 m", "Walls", "models/map-kit/walls/wall_2m.glb", {}),
    ("wall_1m", "Wall 1 m", "Walls", "models/map-kit/walls/wall_1m.glb", {}),
    ("wall_low", "Low cover wall", "Walls", "models/map-kit/walls/wall_low_cover.glb", {}),
    ("wall_window", "Window wall", "Walls", "models/map-kit/walls/wall_window.glb", {}),
    ("wall_door", "Doorway wall", "Walls", "models/map-kit/walls/wall_doorway.glb", {}),
    ("concrete_wall", "Concrete wall", "Walls", "models/map-kit/walls/concrete_wall_2m.glb", {}),
    ("jersey_barrier", "Jersey barrier", "Walls", "models/map-kit/walls/jersey_barrier.glb", {}),
    ("sandbags", "Sandbag wall", "Walls", "models/props/sandbag_wall.glb", {}),
    ("barricade", "Barricade", "Walls", "models/weapons/gadgets/deployable_barricade.glb", {}),
    ("fence_wood", "Wood fence", "Walls", "models/map-kit/structures/fence_wood_2m.glb", {}),
    ("gate", "Gate", "Walls", "models/map-kit/structures/gate_2m.glb", {}),
    ("net_section", "Net section", "Walls", "models/props/netting_fence_section.glb", {}),
    # Structures.
    ("container_green", "Container (green)", "Structures", "models/map-kit/structures/shipping_container_green.glb", {}),
    ("container_rust", "Container (rust)", "Structures", "models/map-kit/structures/shipping_container_rust.glb", {}),
    ("container_blue", "Container (blue)", "Structures", "models/map-kit/structures/shipping_container_blue.glb", {}),
    ("platform", "Platform 2x2", "Structures", "models/map-kit/structures/platform_2x2.glb", {}),
    ("ramp", "Ramp 2 m", "Structures", "models/map-kit/structures/ramp_2m.glb", {}),
    ("stairs", "Stairs 1 m", "Structures", "models/map-kit/structures/stairs_1m.glb", {}),
    ("bridge", "Bridge 4 m", "Structures", "models/map-kit/structures/bridge_4m.glb", {}),
    ("shack", "Field shack", "Structures", "models/map-kit/structures/field_shack.glb", {}),
    ("watch_tower", "Watch tower", "Structures", "models/map-kit/structures/watch_tower.glb", {}),
    ("scoreboard", "Scoreboard", "Structures", "models/map-kit/structures/scoreboard_stand.glb", {}),
    # Props.
    ("barrel", "Barrel", "Props", "models/props/barrel.glb", {}),
    ("crate", "Crate", "Props", "models/props/crate.glb", {}),
    ("pallet_stack", "Pallet stack", "Props", "models/map-kit/props/pallet_stack.glb", {}),
    ("pallet", "Pallet", "Props", "models/map-kit/props/pallet.glb", {}),
    ("tires", "Tire stack", "Props", "models/map-kit/props/tire_stack.glb", {}),
    ("spool", "Cable spool", "Props", "models/map-kit/props/cable_spool.glb", {}),
    ("hay", "Hay bale", "Props", "models/map-kit/props/hay_bale.glb", {}),
    ("log_pile", "Log pile", "Props", "models/map-kit/props/log_pile.glb", {}),
    ("floodlight", "Floodlight", "Props", "models/map-kit/props/floodlight_pole.glb", {}),
    ("sign", "Sign post", "Props", "models/map-kit/props/sign_post.glb", {}),
    ("boundary_post", "Boundary post", "Props", "models/map-kit/props/boundary_post.glb", {}),
    ("cone", "Field cone", "Props", "models/props/field_cone.glb", {}),
    ("grenade_crate", "Grenade crate", "Props", "models/weapons/gadgets/grenade_crate.glb", {}),
    # Nature.
    ("oak", "Oak", "Nature", "models/map-kit/nature/tree_oak.glb", {"decals": False}),
    ("pine", "Pine", "Nature", "models/map-kit/nature/tree_pine.glb", {"decals": False}),
    ("pine_tall", "Tall pine", "Nature", "models/map-kit/nature/tree_pine_tall.glb", {"decals": False}),
    ("bush", "Bush", "Nature", "models/map-kit/nature/bush.glb", {"scenery": True}),
    ("grass", "Grass clump", "Nature", "models/map-kit/nature/grass_clump.glb", {"scenery": True}),
    ("rock_large", "Large rock", "Nature", "models/map-kit/nature/rock_large.glb", {"decals": False}),
    ("rock_medium", "Medium rock", "Nature", "models/map-kit/nature/rock_medium.glb", {"decals": False}),
    ("rock_small", "Small rock", "Nature", "models/map-kit/nature/rock_small.glb", {"decals": False}),
    ("log", "Fallen log", "Nature", "models/map-kit/nature/fallen_log.glb", {"decals": False}),
    ("stump", "Tree stump", "Nature", "models/map-kit/nature/tree_stump.glb", {"decals": False}),
    # Pickups (they don't do anything yet).
    ("paint_case", "Paint case", "Pickups", "models/pickups/paint_case.glb", {}),
    ("pod_pickup", "Pod pickup", "Pickups", "models/pickups/pod_pickup.glb", {}),
    ("air_refill", "Air refill station", "Pickups", "models/pickups/air_refill_station.glb", {}),
]

# Which palette page each new model folder goes on.
FOLDERS = [
    ("models/paintball-field/bunkers", "Bunkers"),
    ("models/map-kit/walls", "Walls"),
    ("models/props/structures", "Walls"),
    ("models/props", "Props"),
    ("models/paintball-field", "Field"),
    ("models/building-blocks", "Building"),
    ("models/interior", "Interior"),
    ("models/industrial", "Industrial"),
    ("models/urban", "Urban"),
    ("models/props/street", "Urban"),
    ("models/environment/trees", "Nature"),
    ("models/environment/plants", "Nature"),
    ("models/nature", "Nature"),
    ("models/environment", "Terrain"),
]

# Better names than the file names give.
LABELS = {
    "big_can": "Big can",
    "big_dorito": "Big dorito",
    "pin": "Pin",
    "snake_corner": "Snake corner",
    "tombstone": "Tombstone",
    "wall_with_ports": "Wall with ports",
    "wedge": "Wedge",
    "wall_corner": "Corner wall",
    "flag_blue": "Flag (blue)",
    "flag_orange": "Flag (orange)",
    "spawn_pad_blue": "Spawn pad (blue)",
    "spawn_pad_orange": "Spawn pad (orange)",
    "respawn_beacon_blue": "Respawn beacon (blue)",
    "respawn_beacon_orange": "Respawn beacon (orange)",
    "team_banner_blue": "Team banner (blue)",
    "team_banner_orange": "Team banner (orange)",
    "tree_big_oak": "Big oak",
    "tree_birch": "Birch",
    "tree_cypress": "Cypress",
    "tree_dead": "Dead tree",
    "tree_palm": "Palm",
    "tree_pine_snowy": "Snowy pine",
    "tree_small_round": "Small round tree",
    "tree_autumn_orange": "Autumn tree (orange)",
    "tree_autumn_red": "Autumn tree (red)",
    "tree_apple": "Apple tree",
    "tree_willow": "Willow",
    "tree_bare_snowy": "Bare snowy tree",
    "hay_bale_round": "Round hay bale",
    "barrel_stack_military": "Military barrel stack",
    "supply_crate_military": "Military supply crate",
    "ammo_box_military": "Military ammo box",
    "oil_drum_rusty": "Rusty oil drum",
    "kitchen_counter_sink": "Kitchen counter",
    "stairs_interior_storey": "Indoor stairs (1 storey)",
    "floor_stair_opening_2x2": "Floor 2x2 (stair hole)",
    "wall_plaster_window_broken": "Plaster wall (broken window)",
    "wall_corrugated_metal_4m": "Corrugated metal wall (4 m)",
    "wall_concrete_fence_2m": "Concrete fence 2 m",
}

# Stems that get no collision at all.
SCENERY = {
    "moss_patch", "flower_patch", "dry_grass_tuft", "tall_grass_patch", "fern", "fallen_leaves",
    "lily_pads", "wheat_patch", "reeds", "sunflowers", "mushroom_cluster", "pebbles", "puddle",
    "dirt_patch", "rug", "manhole_cover", "road_straight_4m", "road_crossing_4m", "capture_point",
    "bush_flowering", "bush_autumn",
}

# Organic shapes: the fit is close but not flat enough for paint splats.
NO_DECALS_DIRS = ("environment/trees", "environment/rocks", "environment/terrain", "nature/", "environment/plants")

OPTIONS = {
    "pond": {"skip": ("3d8fc4_glass",), "decals": False},
    "stream_section_4m": {"skip": ("3d8fc4_glass",), "decals": False},
}

# Older models that were never placeable.
EXTRA = [
    "models/map-kit/walls/wall_corner.glb",
    "models/props/flag_blue.glb",
    "models/props/flag_orange.glb",
    "models/props/spawn_pad_blue.glb",
    "models/props/spawn_pad_orange.glb",
]

# Pieces that belong on another page than their folder's.
CATEGORY = {"water_tower_small": "Structures", "catwalk_2m": "Structures", "metal_stairs": "Structures"}

def label_for(stem):
    if stem in LABELS:
        return LABELS[stem]
    words = stem.split("_")
    out = []
    for w in words:
        if w[:-1].isdigit() and w[-1] == "m":
            out.append(w[:-1] + " m")
        else:
            out.append(w)
    s = " ".join(out)
    return s[0].upper() + s[1:]


def category_for(model):
    best = None
    for folder, cat in FOLDERS:
        if model.startswith(folder + "/") and (best is None or len(folder) > len(best[0])):
            best = (folder, cat)
    return best[1]


def all_pieces(assets):
    """Every piece, in palette order."""
    out = list(EXISTING)
    known = {p[3] for p in out}
    ids = {p[0] for p in out}
    new = list(EXTRA)
    roots = ["models/paintball-field", "models/building-blocks", "models/interior", "models/industrial",
             "models/urban", "models/props", "models/environment", "models/nature"]
    for root in roots:
        for dirpath, _, files in sorted(os.walk(os.path.join(assets, root))):
            for f in sorted(files):
                if f.endswith(".glb"):
                    rel = os.path.relpath(os.path.join(dirpath, f), assets).replace(os.sep, "/")
                    if rel.count("/") >= 3 and rel not in known:
                        new.append(rel)
    for model in new:
        stem = os.path.basename(model)[:-4]
        pid = stem if stem not in ids else stem + "_2"
        assert pid not in ids, pid
        ids.add(pid)
        opts = dict(OPTIONS.get(stem, {}))
        if stem in SCENERY:
            opts["scenery"] = True
        if any(d in model for d in NO_DECALS_DIRS):
            opts.setdefault("decals", False)
        out.append((pid, label_for(stem), CATEGORY.get(stem) or category_for(model), model, opts))
    return out
