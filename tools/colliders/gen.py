"""Fits collision shapes to every placeable model and writes them out as Rust.

    pip install numpy scipy trimesh coacd pillow
    python tools/colliders/gen.py            # writes crates/ink-map/src/pieces.rs
    python tools/colliders/gen.py --sheets   # also draws every model with its shapes

Fitting a model takes from a moment to a minute, so results are kept in
tools/colliders/cache.json and only models that changed are fitted again.
"""

import hashlib
import json
import os
import sys
from multiprocessing import Pool

import numpy as np
from scipy.spatial import ConvexHull
from scipy.spatial.transform import Rotation

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, "..", ".."))
ASSETS = os.path.join(ROOT, "assets")
OUT = os.path.join(ROOT, "crates", "ink-map", "src", "pieces.rs")
CACHE = os.path.join(HERE, "cache.json")

sys.path.insert(0, HERE)
import fit  # noqa: E402
import pieces  # noqa: E402


def _digest(path, opts):
    h = hashlib.sha1()
    h.update(open(path, "rb").read())
    h.update(json.dumps({k: v for k, v in opts.items() if k in ("limit", "skip")}, sort_keys=True).encode())
    return h.hexdigest()


def _plain(s):
    return {k: (v.tolist() if isinstance(v, np.ndarray) else v) for k, v in s.items()}


def _array(s):
    return {k: (np.array(v) if isinstance(v, list) else v) for k, v in s.items()}


def fit_one(job):
    model, opts = job
    shapes = fit.model_shapes(os.path.join(ASSETS, model), limit=opts.get("limit", 16), skip_materials=opts.get("skip", ()))
    return model, [_plain(s) for s in shapes]


def f(v):
    r = round(float(v), 3)
    if r == 0:
        r = 0.0
    s = repr(r)
    return s


def vec(v):
    return "[" + ", ".join(f(x) for x in v) + "]"


def hull_rust(points):
    """Corners, faces (n, d: n.p <= d) and edges of a convex hull."""
    pts = np.round(np.asarray(points), 3)
    h = ConvexHull(pts)
    corners = list(h.vertices)
    index = {v: i for i, v in enumerate(corners)}
    planes = []
    for eq in h.equations:
        n = np.round(eq[:3], 5)
        # Faces pass through the (rounded) corners, so every corner is inside.
        d = float((pts @ n).max())
        p = (n[0], n[1], n[2], d)
        if not any(np.allclose(p, q, atol=2e-3) for q in planes):
            planes.append(p)
    edges = set()
    for i, simp in enumerate(h.simplices):
        for k in range(3):
            a, b = sorted((simp[k], simp[(k + 1) % 3]))
            edges.add((a, b))
    keep = []
    for a, b in sorted(edges):
        faces = [i for i, sm in enumerate(h.simplices) if a in sm and b in sm]
        if len(faces) == 2 and np.allclose(h.equations[faces[0]], h.equations[faces[1]], atol=2e-3):
            continue
        keep.append((index[a], index[b]))
    return [pts[c] for c in corners], planes, keep


def part_rust(s, hulls):
    k = s["kind"]
    if k == "box":
        return f"B({vec(s['min'])}, {vec(s['max'])})"
    if k == "cyl":
        return f"C({f(s['x'])}, {f(s['z'])}, {f(s['r'])}, {f(s['y0'])}, {f(s['h'])})"
    if k == "turned":
        rot = np.asarray(s["rot"])
        if np.allclose(np.abs(rot), np.eye(3), atol=1e-6):
            c, h = np.asarray(s["center"]), np.asarray(s["half"])
            return f"B({vec(c - h)}, {vec(c + h)})"
        q = Rotation.from_matrix(rot).as_quat()
        return f"T({vec(s['center'])}, {vec(s['half'])}, [{', '.join(repr(round(float(x), 5)) for x in q)}])"
    if k == "rod":
        return f"Rod({vec(s['a'])}, {vec(s['b'])}, {f(s['r'])})"
    corners, planes, edges = hull_rust(s["points"])
    name = f"H{len(hulls)}"
    hulls.append(
        f"static {name}: Hull = Hull {{\n"
        f"    points: &[{', '.join(vec(p) for p in corners)}],\n"
        f"    planes: &[{', '.join('[' + ', '.join(repr(round(float(x), 5) or 0.0) for x in p) + ']' for p in planes)}],\n"
        f"    edges: &[{', '.join(f'[{a}, {b}]' for a, b in edges)}],\n"
        f"}};"
    )
    return f"H(&{name})"


def main():
    table = pieces.all_pieces(ASSETS)
    cache = json.load(open(CACHE)) if os.path.exists(CACHE) else {}
    jobs, digests = [], {}
    for pid, label, cat, model, opts in table:
        if opts.get("scenery") or "parts" in opts:
            continue
        d = _digest(os.path.join(ASSETS, model), opts)
        digests[model] = d
        if cache.get(model, {}).get("digest") != d:
            jobs.append((model, opts))
    if jobs:
        print(f"fitting {len(jobs)} models...", flush=True)
        with Pool(max(1, os.cpu_count() or 1)) as pool:
            for model, shapes in pool.imap_unordered(fit_one, jobs):
                cache[model] = {"digest": digests[model], "shapes": shapes}
                print("  ", model, len(shapes), flush=True)
                json.dump(cache, open(CACHE, "w"))
    hulls, rows = [], []
    for pid, label, cat, model, opts in table:
        if opts.get("scenery"):
            parts = ""
        elif "parts" in opts:
            parts = opts["parts"]
        else:
            parts = ", ".join(part_rust(_array(s), hulls) for s in cache[model]["shapes"])
        decals = "true" if opts.get("decals", True) and not opts.get("scenery") else "false"
        rows.append(f'    p!("{pid}", "{label}", {cat}, "{model}", {decals}, [{parts}]),')
    text = (
        "// Written by tools/colliders/gen.py from tools/colliders/pieces.py and the\n"
        "// models themselves. Don't edit by hand: change those and run it again.\n\n"
        "use crate::catalog::Category::*;\n"
        "use crate::catalog::Part::{Box as B, Cyl as C, Hull as H, Rod, Turned as T};\n"
        "use crate::catalog::{Hull, Piece};\n\n"
        "macro_rules! p {\n"
        "    ($id:literal, $label:literal, $cat:expr, $model:literal, $decals:expr, [$($part:expr),* $(,)?]) => {\n"
        "        Piece { id: $id, label: $label, category: $cat, model: $model, parts: &[$($part),*], decals: $decals }\n"
        "    };\n"
        "}\n\n"
        "/// Every piece, in palette order.\n"
        "#[rustfmt::skip]\n"
        "pub static PIECES: &[Piece] = &[\n" + "\n".join(rows) + "\n];\n\n" + "\n".join(hulls) + "\n"
    )
    open(OUT, "w", newline="\n").write(text)
    print(f"wrote {OUT}: {len(rows)} pieces, {len(hulls)} hulls")
    if "--sheets" in sys.argv:
        sheets(table, cache)


def sheets(table, cache, per=12, cols=3):
    from PIL import Image

    from render import render

    out_dir = os.path.join(HERE, "sheets")
    os.makedirs(out_dir, exist_ok=True)
    items = [(pid, model) for pid, _, _, model, opts in table if model in cache and not opts.get("scenery")]
    for n in range(0, len(items), per):
        imgs = []
        for pid, model in items[n : n + per]:
            shapes = [_array(s) for s in cache[model]["shapes"]]
            imgs.append(render(os.path.join(ASSETS, model), shapes, size=200, label=f"{pid} ({len(shapes)})"))
        w, h = imgs[0].size
        rows = (len(imgs) + cols - 1) // cols
        sheet = Image.new("RGB", (w * cols, h * rows), "white")
        for i, im in enumerate(imgs):
            sheet.paste(im, ((i % cols) * w, (i // cols) * h))
        sheet.save(os.path.join(out_dir, f"sheet_{n // per:02d}.png"))


if __name__ == "__main__":
    main()
