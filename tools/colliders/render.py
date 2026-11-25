"""Draws a model with its collision shapes over it, to check them by eye."""
import numpy as np
from PIL import Image, ImageDraw, ImageFont

from fit import load_glb, shape_points

EDGES = {}


def shape_edges(s):
    from scipy.spatial import ConvexHull

    pts = shape_points(s)
    if s["kind"] in ("box", "turned"):
        idx = [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)]
        return pts, idx
    if s["kind"] in ("cyl", "rod"):
        n = 12
        idx = [(i, (i + 1) % n) for i in range(n)] + [(n + i, n + (i + 1) % n) for i in range(n)] + [(i, n + i) for i in range(0, n, 3)]
        return pts, idx
    h = ConvexHull(pts)
    edges = set()
    # Only edges between faces that aren't coplanar.
    eq = h.equations
    for si, simp in enumerate(h.simplices):
        for k in range(3):
            a, b = sorted((simp[k], simp[(k + 1) % 3]))
            edges.add((a, b))
    keep = []
    for a, b in edges:
        faces = [i for i, sm in enumerate(h.simplices) if a in sm and b in sm]
        if len(faces) == 2 and np.allclose(eq[faces[0]], eq[faces[1]], atol=1e-4):
            continue
        keep.append((a, b))
    return pts, keep


def view(tris, shapes, yaw, pitch, size, label):
    cy, sy, cp, sp = np.cos(yaw), np.sin(yaw), np.cos(pitch), np.sin(pitch)
    R = np.array([[cy, 0, -sy], [0, 1, 0], [sy, 0, cy]])
    P = np.array([[1, 0, 0], [0, cp, -sp], [0, sp, cp]])
    M = P @ R
    allp = tris.reshape(-1, 3)
    for s in shapes:
        allp = np.vstack([allp, shape_points(s)])
    q = allp @ M.T
    lo, hi = q.min(0), q.max(0)
    scale = (size - 20) / max(hi[0] - lo[0], hi[1] - lo[1], 1e-3)
    mid = (lo + hi) / 2

    def proj(p):
        r = p @ M.T
        return np.stack([(r[:, 0] - mid[0]) * scale + size / 2, size / 2 - (r[:, 1] - mid[1]) * scale, r[:, 2]], 1)

    img = Image.new("RGB", (size, size), (250, 250, 250))
    zb = np.full((size, size), -1e9)
    pix = np.array(img)
    light = np.array([0.4, 0.8, 0.45])
    light /= np.linalg.norm(light)
    pt = proj(tris.reshape(-1, 3)).reshape(-1, 3, 3)
    n = np.cross(tris[:, 1] - tris[:, 0], tris[:, 2] - tris[:, 0])
    nn = np.linalg.norm(n, axis=1, keepdims=True)
    n = n / np.maximum(nn, 1e-12)
    shade = 0.45 + 0.5 * np.abs(n @ light)
    for t, sh in zip(pt, shade):
        x0, y0 = np.floor(t[:, :2].min(0)).astype(int)
        x1, y1 = np.ceil(t[:, :2].max(0)).astype(int)
        x0, y0 = max(x0, 0), max(y0, 0)
        x1, y1 = min(x1, size - 1), min(y1, size - 1)
        if x1 < x0 or y1 < y0:
            continue
        xs, ys = np.meshgrid(np.arange(x0, x1 + 1) + 0.5, np.arange(y0, y1 + 1) + 0.5)
        (ax, ay, az), (bx, by, bz), (cx, cy_, cz) = t
        den = (by - cy_) * (ax - cx) + (cx - bx) * (ay - cy_)
        if abs(den) < 1e-9:
            continue
        w0 = ((by - cy_) * (xs - cx) + (cx - bx) * (ys - cy_)) / den
        w1 = ((cy_ - ay) * (xs - cx) + (ax - cx) * (ys - cy_)) / den
        w2 = 1 - w0 - w1
        inside = (w0 >= -1e-6) & (w1 >= -1e-6) & (w2 >= -1e-6)
        z = w0 * az + w1 * bz + w2 * cz
        sub = zb[y0 : y1 + 1, x0 : x1 + 1]
        m = inside & (z > sub)
        sub[m] = z[m]
        g = int(200 * sh)
        pix[y0 : y1 + 1, x0 : x1 + 1][m] = (g, g, int(g * 0.95))
    img = Image.fromarray(pix)
    d = ImageDraw.Draw(img)
    colors = [(220, 30, 30), (30, 90, 220), (20, 160, 60), (200, 120, 0), (150, 40, 180)]
    for k, s in enumerate(shapes):
        pts, edges = shape_edges(s)
        pp = proj(pts)
        for a, b in edges:
            d.line([tuple(pp[a, :2]), tuple(pp[b, :2])], fill=colors[k % len(colors)], width=1)
    d.text((4, 2), label, fill=(0, 0, 0))
    return img


def render(path, shapes, size=260, label=""):
    prims = load_glb(path)
    tris = np.vstack([t for _, t in prims])
    a = view(tris, shapes, np.radians(35), np.radians(25), size, label)
    b = view(tris, shapes, np.radians(-125), np.radians(20), size, "")
    out = Image.new("RGB", (size * 2, size), (255, 255, 255))
    out.paste(a, (0, 0))
    out.paste(b, (size, 0))
    return out
