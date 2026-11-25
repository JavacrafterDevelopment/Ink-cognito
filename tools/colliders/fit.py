"""Fits collision shapes to a .glb model.

The model is split into its separate solid pieces (the parts of a low-poly
model are separate closed meshes: a post, a plank, a bunker body). Pieces that
aren't convex are broken into convex parts with CoACD. Each convex part then
gets the simplest shape that hugs it: an axis-aligned box, an upright
cylinder, a turned box, a rod (a cylinder along any axis) or, failing those,
its convex hull. Parts that fit together without adding empty space are
merged, and if a model still has too many, the merges that add the least empty
space are made until it's under the limit.

Everything here is in the model's own space: meters, Y up, origin on the
ground under the piece.
"""

import json
import struct
import warnings

import numpy as np
import trimesh
from scipy.spatial import ConvexHull, cKDTree

warnings.filterwarnings("ignore")

# How far (meters) a convex shape may bulge past the model before the piece
# counts as concave and is split.
CONCAVE_TOL = 0.05
# A primitive must fill at least this much of what it stands for.
FILL = 0.88
# Thinner than this, a flat piece (a sign face, a cloth) gets this thickness.
MIN_THICK = 0.03
# Quick mode: curved pieces are split by clustering instead of CoACD
# (looser, but seconds instead of minutes).
FAST = True
# Free merges may add at most this much empty space (cubic meters).
MAX_ADDED = 0.01
# Convex hulls are cut down to at most this many corners.
MAX_HULL_POINTS = 24


# --- Loading ---------------------------------------------------------------


def _quat_mat(q):
    x, y, z, w = q
    return np.array(
        [
            [1 - 2 * (y * y + z * z), 2 * (x * y - z * w), 2 * (x * z + y * w)],
            [2 * (x * y + z * w), 1 - 2 * (x * x + z * z), 2 * (y * z - x * w)],
            [2 * (x * z - y * w), 2 * (y * z + x * w), 1 - 2 * (x * x + y * y)],
        ]
    )


def _node_mat(n):
    if "matrix" in n:
        return np.array(n["matrix"]).reshape(4, 4).T
    m = np.eye(4)
    m[:3, :3] = _quat_mat(n.get("rotation", [0, 0, 0, 1])) @ np.diag(n.get("scale", [1, 1, 1]))
    m[:3, 3] = n.get("translation", [0, 0, 0])
    return m


def load_glb(path):
    """Every triangle of the model's default scene, as (material, triangles[n,3,3])."""
    data = open(path, "rb").read()
    _, _, length = struct.unpack("<III", data[:12])
    off, js, binary = 12, None, None
    while off < length:
        clen, ctype = struct.unpack("<II", data[off : off + 8])
        chunk = data[off + 8 : off + 8 + clen]
        if ctype == 0x4E4F534A:
            js = json.loads(chunk)
        else:
            binary = chunk
        off += 8 + clen

    def accessor(i):
        a = js["accessors"][i]
        bv = js["bufferViews"][a["bufferView"]]
        dtype = {5126: np.float32, 5123: np.uint16, 5125: np.uint32, 5121: np.uint8}[a["componentType"]]
        n = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4}[a["type"]]
        size = np.dtype(dtype).itemsize
        start = bv.get("byteOffset", 0) + a.get("byteOffset", 0)
        stride = bv.get("byteStride", size * n)
        raw = np.frombuffer(binary, dtype=np.uint8, count=stride * (a["count"] - 1) + size * n, offset=start)
        rows = np.lib.stride_tricks.as_strided(raw, shape=(a["count"], size * n), strides=(stride, 1))
        return np.frombuffer(rows.copy().tobytes(), dtype=dtype).reshape(a["count"], n).astype(np.float64)

    out = []

    def walk(ni, parent):
        node = js["nodes"][ni]
        m = parent @ _node_mat(node)
        if "mesh" in node:
            for prim in js["meshes"][node["mesh"]]["primitives"]:
                if prim.get("mode", 4) != 4:
                    continue
                v = accessor(prim["attributes"]["POSITION"])
                v = v @ m[:3, :3].T + m[:3, 3]
                idx = accessor(prim["indices"]).astype(np.int64).flatten() if "indices" in prim else np.arange(len(v))
                mat = ""
                if "material" in prim and "materials" in js:
                    mat = js["materials"][prim["material"]].get("name", "")
                out.append((mat, v[idx.reshape(-1, 3)]))
        for c in node.get("children", []):
            walk(c, m)

    scene = js["scenes"][js.get("scene", 0)]
    for r in scene["nodes"]:
        walk(r, np.eye(4))
    return out


def pieces(prims):
    """Splits triangles into connected pieces (corners in the same place join)."""
    result = []
    for mat, tris in prims:
        flat = tris.reshape(-1, 3)
        key = np.round(flat / 1e-4).astype(np.int64)
        _, inv = np.unique(key, axis=0, return_inverse=True)
        inv = inv.reshape(-1, 3)
        n = inv.max() + 1
        parent = np.arange(n)

        def find(a):
            root = a
            while parent[root] != root:
                root = parent[root]
            while parent[a] != root:
                parent[a], a = root, parent[a]
            return root

        for a, b, c in inv:
            ra, rb, rc = find(a), find(b), find(c)
            parent[rb] = ra
            parent[find(rc)] = ra
        roots = np.array([find(i) for i in range(n)])
        tri_root = roots[inv[:, 0]]
        for r in np.unique(tri_root):
            result.append(dict(mat=mat, tris=tris[tri_root == r]))
    return result


# --- Convex parts ----------------------------------------------------------


def surface_points(tris, count):
    """Points spread over the triangles (plus their corners)."""
    a, b, c = tris[:, 0], tris[:, 1], tris[:, 2]
    area = 0.5 * np.linalg.norm(np.cross(b - a, c - a), axis=1)
    pts = [tris.reshape(-1, 3)]
    if area.sum() > 1e-9:
        rng = np.random.default_rng(7)
        pick = rng.choice(len(tris), size=count, p=area / area.sum())
        u, v = rng.random(count), rng.random(count)
        flip = u + v > 1
        u[flip], v[flip] = 1 - u[flip], 1 - v[flip]
        pts.append(a[pick] + (b[pick] - a[pick]) * u[:, None] + (c[pick] - a[pick]) * v[:, None])
    return np.vstack(pts)


def hull_of(points):
    try:
        return ConvexHull(points)
    except Exception:
        return None


def concavity(tris, points):
    """How far (meters) the convex hull of a piece bulges past its surface."""
    h = hull_of(points)
    if h is None:
        return 0.0
    hull_tris = points[h.simplices]
    probe = surface_points(hull_tris, 3000)
    surf = surface_points(tris, 12000)
    d, _ = cKDTree(surf).query(probe)
    return float(np.percentile(d, 99))


def boxy_parts(tris):
    """Splits a piece whose faces all face along the axes (a wall with a door
    cut out of it, an L-shaped floor) into the boxes it's made of, exactly.
    None if it isn't one, or the pieces don't close up."""
    a, b, c = tris[:, 0], tris[:, 1], tris[:, 2]
    n = np.cross(b - a, c - a)
    ln = np.linalg.norm(n, axis=1)
    tris, n = tris[ln > 1e-12], n[ln > 1e-12] / ln[ln > 1e-12, None]
    if len(tris) == 0 or not np.all(np.abs(n).max(1) > 0.999):
        return None
    coords = [np.unique(np.round(tris[:, :, k], 4)) for k in range(3)]
    if min(len(c) for c in coords) < 2 or np.prod([len(c) - 1 for c in coords]) > 60000:
        return None
    mids = [(c[1:] + c[:-1]) / 2 for c in coords]
    # Faces across x: a cell is inside if a ray toward +x crosses an odd number.
    fx = tris[np.abs(n[:, 0]) > 0.999]
    filled = np.zeros([len(m) for m in mids], dtype=bool)
    for j, y in enumerate(mids[1]):
        for k, z in enumerate(mids[2]):
            p = np.array([y, z])
            t = fx[:, :, 1:]
            v0, v1, v2 = t[:, 0], t[:, 1], t[:, 2]
            d = (v1[:, 1] - v2[:, 1]) * (v0[:, 0] - v2[:, 0]) + (v2[:, 0] - v1[:, 0]) * (v0[:, 1] - v2[:, 1])
            ok = np.abs(d) > 1e-12
            w0 = ((v1[:, 1] - v2[:, 1]) * (p[0] - v2[:, 0]) + (v2[:, 0] - v1[:, 0]) * (p[1] - v2[:, 1])) / np.where(ok, d, 1)
            w1 = ((v2[:, 1] - v0[:, 1]) * (p[0] - v2[:, 0]) + (v0[:, 0] - v2[:, 0]) * (p[1] - v2[:, 1])) / np.where(ok, d, 1)
            w2 = 1 - w0 - w1
            hit = ok & (w0 >= 0) & (w1 >= 0) & (w2 >= 0)
            xs = np.unique(np.round(fx[hit][:, 0, 0], 4))
            if len(xs) % 2:
                return None
            for i, x in enumerate(mids[0]):
                filled[i, j, k] = (xs > x).sum() % 2 == 1
    if not filled.any():
        return None
    # Greedy: grow each box along x, then z, then y.
    used = np.zeros_like(filled)
    out = []
    nx, ny, nz = filled.shape
    for j in range(ny):
        for k in range(nz):
            for i in range(nx):
                if not filled[i, j, k] or used[i, j, k]:
                    continue
                i1 = i
                while i1 + 1 < nx and filled[i1 + 1, j, k] and not used[i1 + 1, j, k]:
                    i1 += 1
                k1 = k
                while k1 + 1 < nz and (filled[i : i1 + 1, j, k1 + 1] & ~used[i : i1 + 1, j, k1 + 1]).all():
                    k1 += 1
                j1 = j
                while j1 + 1 < ny and (filled[i : i1 + 1, j1 + 1, k : k1 + 1] & ~used[i : i1 + 1, j1 + 1, k : k1 + 1]).all():
                    j1 += 1
                used[i : i1 + 1, j : j1 + 1, k : k1 + 1] = True
                lo = np.array([coords[0][i], coords[1][j], coords[2][k]])
                hi = np.array([coords[0][i1 + 1], coords[1][j1 + 1], coords[2][k1 + 1]])
                out.append(np.array([[x, y, z] for x in (lo[0], hi[0]) for y in (lo[1], hi[1]) for z in (lo[2], hi[2])]))
    return out


def cluster_parts(tris, count=6):
    """A quick split of a curved piece: its surface points grouped into
    `count` clumps (k-means), each clump's hull one part."""
    from scipy.cluster.vq import kmeans2

    pts = surface_points(tris, 1500)
    k = min(count, max(2, len(pts) // 20))
    _, label = kmeans2(pts, k, minit="++", seed=3)
    out = [pts[label == i] for i in range(k) if (label == i).sum() >= 4]
    return out or [pts]


def convex_parts(tris):
    """The convex parts of one piece, as point sets."""
    points = np.unique(np.round(tris.reshape(-1, 3), 5), axis=0)
    small = (points.max(0) - points.min(0)).max() < 0.4
    if len(points) < 4 or small or concavity(tris, points) <= CONCAVE_TOL:
        return [points]
    boxes = boxy_parts(tris)
    if boxes is not None:
        return boxes
    if FAST:
        return cluster_parts(tris)
    import coacd

    coacd.set_log_level("error")
    m = trimesh.Trimesh(vertices=tris.reshape(-1, 3), faces=np.arange(len(tris) * 3).reshape(-1, 3))
    m.merge_vertices()
    try:
        res = coacd.run_coacd(
            coacd.Mesh(m.vertices, m.faces),
            threshold=0.04,
            preprocess_mode="auto",
            max_convex_hull=12,
            resolution=1000,
            mcts_iterations=60,
            mcts_max_depth=2,
            seed=1,
        )
    except Exception:
        return [points]
    return [np.asarray(v, dtype=np.float64) for v, _ in res if len(v) >= 1]


# --- Shapes ----------------------------------------------------------------


def _pad(lo, hi):
    """Gives flat boxes a little thickness."""
    lo, hi = lo.copy(), hi.copy()
    for a in range(3):
        if hi[a] - lo[a] < MIN_THICK:
            mid = (hi[a] + lo[a]) / 2
            lo[a], hi[a] = mid - MIN_THICK / 2, mid + MIN_THICK / 2
    return lo, hi


def hull_volume(points):
    h = hull_of(points)
    return h.volume if h is not None else 0.0


def fit_box(points):
    lo, hi = _pad(points.min(0), points.max(0))
    return dict(kind="box", min=lo, max=hi, vol=float(np.prod(hi - lo)))


def fit_cylinder(points):
    """Upright cylinder, if the points sit on a circle seen from above."""
    lo, hi = points.min(0), points.max(0)
    c = (lo[[0, 2]] + hi[[0, 2]]) / 2
    r = np.linalg.norm(points[:, [0, 2]] - c, axis=1)
    rmax = r.max()
    if rmax < 1e-3:
        return None
    rim = r > rmax * 0.8
    angles = np.unique(np.round(np.degrees(np.arctan2(*(points[rim][:, [2, 0]] - c).T)) / 10))
    if len(angles) < 8 or (rmax - r[rim].min()) / rmax > 0.12:
        return None
    # Between the corners and the middles of the sides of the polygon.
    radius = float(rmax * 0.985)
    h = max(hi[1] - lo[1], MIN_THICK)
    return dict(kind="cyl", x=float(c[0]), z=float(c[1]), r=radius, y0=float(lo[1]), h=float(h), vol=float(np.pi * radius**2 * h))


def _pca_frame(points):
    centered = points - points.mean(0)
    _, _, vt = np.linalg.svd(centered, full_matrices=False)
    return vt


def _snap_axes(rot):
    """Rounds near-axis directions so boxes that are lined up stay lined up."""
    rot = rot.copy()
    for i in range(3):
        col = rot[:, i]
        big = np.argmax(np.abs(col))
        if abs(col[big]) > 0.9995:
            sign = np.sign(col[big])
            col[:] = 0
            col[big] = sign
    return rot


def fit_turned_box(points):
    """The smallest box around the points, turned any way."""
    if len(points) < 4 or hull_of(points) is None:
        vt = _pca_frame(points)
        rot = vt.T
    else:
        tf, _ = trimesh.bounds.oriented_bounds(points)
        rot = np.linalg.inv(tf)[:3, :3]
    rot = _snap_axes(rot)
    if np.linalg.det(rot) < 0:
        rot[:, 2] *= -1
    local = points @ rot
    lo, hi = _pad(local.min(0), local.max(0))
    center = rot @ ((lo + hi) / 2)
    half = (hi - lo) / 2
    return dict(kind="turned", center=center, half=half, rot=rot, vol=float(np.prod(hi - lo)))


def fit_rod(points):
    """A cylinder along the points' long axis, if they sit on a circle around it."""
    vt = _pca_frame(points)
    axis = vt[0]
    mean = points.mean(0)
    t = (points - mean) @ axis
    radial = (points - mean) - np.outer(t, axis)
    # Center the circle on the middle of the extremes across the axis.
    u, w = vt[1], vt[2]
    pu, pw = radial @ u, radial @ w
    cu, cw = (pu.max() + pu.min()) / 2, (pw.max() + pw.min()) / 2
    r = np.hypot(pu - cu, pw - cw)
    rmax = r.max()
    length = t.max() - t.min()
    if rmax < 1e-3 or length < 2 * rmax * 0.6:
        return None
    rim = r > rmax * 0.8
    ang = np.unique(np.round(np.degrees(np.arctan2(pw[rim] - cw, pu[rim] - cu)) / 10))
    if len(ang) < 8 or (rmax - r[rim].min()) / rmax > 0.12:
        return None
    center = mean + u * cu + w * cw
    a = center + axis * t.min()
    b = center + axis * t.max()
    radius = float(rmax * 0.985)
    return dict(kind="rod", a=a, b=b, r=radius, vol=float(np.pi * radius**2 * length))


def simplify_hull(points, limit=MAX_HULL_POINTS):
    """Hull corners, cut down to `limit` by dropping the ones that matter least."""
    h = hull_of(points)
    if h is None:
        return points
    pts = points[h.vertices]
    while len(pts) > limit:
        full = hull_volume(pts)
        best, best_loss = None, None
        for i in range(len(pts)):
            loss = full - hull_volume(np.delete(pts, i, axis=0))
            if best_loss is None or loss < best_loss:
                best, best_loss = i, loss
        pts = np.delete(pts, best, axis=0)
    # Dropping corners shrinks the hull; push it back out so it still covers.
    h = ConvexHull(pts)
    eq = h.equations
    over = (points @ eq[:, :3].T + eq[:, 3]).max(0)
    eq = eq.copy()
    eq[:, 3] -= np.maximum(over, 0)
    return hull_from_planes(eq, pts)


def hull_from_planes(eq, guess):
    """Corners of the shape cut out by the planes n.x + d <= 0."""
    try:
        from scipy.spatial import HalfspaceIntersection

        hs = HalfspaceIntersection(eq, guess.mean(0))
        return np.unique(np.round(hs.intersections, 5), axis=0)
    except Exception:
        return guess


def fit_hull(points):
    pts = points
    if hull_of(points) is None:
        t = fit_turned_box(points)
        return t
    pts = simplify_hull(points)
    h = hull_of(pts)
    if h is None:
        return fit_turned_box(points)
    return dict(kind="hull", points=pts, vol=float(h.volume))


def fit(points):
    """The simplest shape that hugs the points."""
    vol = hull_volume(points)
    box = fit_box(points)
    if vol <= 1e-7:
        # Flat: a thin box.
        t = fit_turned_box(points)
        return box if box["vol"] <= t["vol"] * 1.05 else t
    for f in (lambda p: box, fit_cylinder, fit_turned_box, fit_rod):
        s = f(points)
        if s is not None and vol / s["vol"] >= FILL:
            s["fill"] = vol / s["vol"]
            return s
    s = fit_hull(points)
    s["fill"] = 1.0
    return s


def shape_points(s):
    """Points that span a shape (for merging)."""
    if s["kind"] == "box":
        lo, hi = s["min"], s["max"]
        return np.array([[x, y, z] for x in (lo[0], hi[0]) for y in (lo[1], hi[1]) for z in (lo[2], hi[2])])
    if s["kind"] == "turned":
        signs = np.array(list(np.ndindex(2, 2, 2))) * 2 - 1
        return s["center"] + (signs * s["half"]) @ s["rot"].T
    if s["kind"] == "cyl":
        a = np.linspace(0, 2 * np.pi, 12, endpoint=False)
        ring = np.stack([s["x"] + s["r"] * np.cos(a), np.zeros_like(a), s["z"] + s["r"] * np.sin(a)], 1)
        top = ring.copy()
        ring[:, 1] = s["y0"]
        top[:, 1] = s["y0"] + s["h"]
        return np.vstack([ring, top])
    if s["kind"] == "rod":
        axis = s["b"] - s["a"]
        axis = axis / np.linalg.norm(axis)
        u = np.cross(axis, [0, 1, 0] if abs(axis[1]) < 0.9 else [1, 0, 0])
        u /= np.linalg.norm(u)
        w = np.cross(axis, u)
        a = np.linspace(0, 2 * np.pi, 12, endpoint=False)
        ring = (np.outer(np.cos(a), u) + np.outer(np.sin(a), w)) * s["r"]
        return np.vstack([s["a"] + ring, s["b"] + ring])
    return s["points"]


# --- Whole models ----------------------------------------------------------


def _overlaps(a, b, gap):
    return np.all(a[0] <= b[1] + gap) and np.all(b[0] <= a[1] + gap)


def _hull_points(pts):
    h = hull_of(pts)
    return pts[h.vertices] if h is not None else pts


def merge(parts, limit, loose=1.06):
    """Joins parts that fit one shape with no extra empty space, then (over
    `limit`) the ones that add the least."""
    items = []

    def item(pts, s):
        sp = shape_points(s)
        return dict(points=_hull_points(pts), shape=s, box=(sp.min(0), sp.max(0)), id=object())

    items = [item(p, s) for p, s in parts]
    cache = {}

    def union(a, b):
        key = (id(a["id"]), id(b["id"]))
        if key not in cache:
            pts = _hull_points(np.vstack([a["points"], b["points"]]))
            cache[key] = (pts, fit(pts))
        return cache[key]

    def replace(i, j, pts, s):
        items[i] = item(pts, s)
        del items[j]

    while len(items) > 1:
        best = None
        for i in range(len(items)):
            for j in range(i + 1, len(items)):
                a, b = items[i], items[j]
                if not _overlaps(a["box"], b["box"], 0.02):
                    continue
                pts, s = union(a, b)
                both = a["shape"]["vol"] + b["shape"]["vol"]
                cost = s["vol"] / max(both, 1e-9)
                # Little extra space, and none of it in a lump (a tap joined
                # to a counter would put a sloped roof over the counter).
                if cost <= loose and s["vol"] - both <= MAX_ADDED and (best is None or cost < best[0]):
                    best = (cost, i, j, pts, s)
        if best is None:
            break
        replace(*best[1:])
    while len(items) > limit:
        best = None
        for i in range(len(items)):
            for j in range(i + 1, len(items)):
                a, b = items[i], items[j]
                pts, s = union(a, b)
                added = s["vol"] - a["shape"]["vol"] - b["shape"]["vol"]
                if best is None or added < best[0]:
                    best = (added, i, j, pts, s)
        replace(*best[1:])
    return [(it["points"], it["shape"]) for it in items]


def absorb(parts, tol=0.025):
    """Leaves out parts that are within `tol` of another part's shape (a
    plank lying on a deck, a trim strip on a wall)."""
    shapes = [s for _, s in parts]
    planes = []
    for s in shapes:
        h = hull_of(shape_points(s))
        planes.append(h.equations if h is not None else None)
    keep = []
    order = sorted(range(len(parts)), key=lambda i: shapes[i]["vol"], reverse=True)
    kept_idx = []
    for i in order:
        pts = shape_points(shapes[i])
        inside = False
        for j in kept_idx:
            eq = planes[j]
            if eq is not None and ((pts @ eq[:, :3].T + eq[:, 3]).max() <= tol):
                inside = True
                break
        if not inside:
            kept_idx.append(i)
    return [parts[i] for i in sorted(kept_idx)]


def drop_tiny(parts, size=0.03):
    """Leaves out bits smaller than `size` across (bolts, studs)."""
    kept = [(p, s) for p, s in parts if (p.max(0) - p.min(0)).max() >= size]
    return kept or parts


def model_shapes(path, limit=12, skip_materials=()):
    prims = [(m, t) for m, t in load_glb(path) if not any(k in m.lower() for k in skip_materials)]
    parts = []
    for piece in pieces(prims):
        for pts in convex_parts(piece["tris"]):
            if len(pts) == 0:
                continue
            parts.append((pts, fit(pts)))
    parts = absorb(drop_tiny(parts))
    return [s for _, s in merge(parts, limit)]
