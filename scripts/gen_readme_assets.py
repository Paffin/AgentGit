#!/usr/bin/env python3
"""Generates the README's SVG artwork (docs/assets/hero.svg, hero.ru.svg and architecture.svg).

    python3 scripts/gen_readme_assets.py           # (re)write the files
    python3 scripts/gen_readme_assets.py --check   # fail if the checked-in files are out of date

Both images carry their own background so they read the same on GitHub's light and dark themes, and use
system fonts only (GitHub does not load web fonts for images).
"""
import pathlib
import sys
from xml.sax.saxutils import escape

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / "docs" / "assets"
FONT = "-apple-system, BlinkMacSystemFont, 'Segoe UI', Inter, Roboto, Helvetica, Arial, sans-serif"
MONO = "ui-monospace, SFMono-Regular, Menlo, Consolas, 'Liberation Mono', monospace"


def text(x, y, s, size=14, fill="#1f2328", weight=400, anchor="start", family=FONT, extra=""):
    return (
        f'<text x="{x}" y="{y}" font-family="{family}" font-size="{size}" font-weight="{weight}" '
        f'fill="{fill}" text-anchor="{anchor}" {extra}>{escape(s)}</text>'
    )


def rect(x, y, w, h, fill, stroke=None, rx=10, sw=1, dash=None, extra=""):
    st = f' stroke="{stroke}" stroke-width="{sw}"' if stroke else ""
    da = f' stroke-dasharray="{dash}"' if dash else ""
    return f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{rx}" fill="{fill}"{st}{da} {extra}/>'


def chip(x, y, label, fill, stroke, color, size=13, pad=11, h=28, mono=False):
    """A pill whose width follows its label; returns (svg, width)."""
    cw = size * (0.60 if mono else 0.56)
    w = int(len(label) * cw + 2 * pad)
    fam = MONO if mono else FONT
    svg = rect(x, y, w, h, fill, stroke, rx=h // 2) + text(
        x + w / 2, y + h / 2 + size * 0.35, label, size, color, 600, "middle", fam
    )
    return svg, w


def chips_row(x, y, labels, fill, stroke, color, gap=8, max_w=None, **kw):
    out, cx, cy = [], x, y
    for lab in labels:
        svg, w = chip(cx, cy, lab, fill, stroke, color, **kw)
        if max_w and cx + w > x + max_w:
            cx, cy = x, cy + kw.get("h", 28) + gap
            svg, w = chip(cx, cy, lab, fill, stroke, color, **kw)
        out.append(svg)
        cx += w + gap
    return "".join(out), cy + kw.get("h", 28)


def arrow(x1, y1, x2, y2, color="#57606a", dash=None, width=2):
    da = f' stroke-dasharray="{dash}"' if dash else ""
    ang_down = y2 > y1
    head = (
        f'<path d="M{x2 - 6} {y2 - 9 if ang_down else y2 + 9} L{x2} {y2} L{x2 + 6} {y2 - 9 if ang_down else y2 + 9}" '
        f'fill="none" stroke="{color}" stroke-width="{width}" stroke-linecap="round" stroke-linejoin="round"/>'
    )
    return f'<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" stroke="{color}" stroke-width="{width}"{da} stroke-linecap="round"/>' + head


def svg_doc(w, h, body, title, desc):
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}" role="img" '
        f'aria-labelledby="t d"><title id="t">{escape(title)}</title><desc id="d">{escape(desc)}</desc>{body}</svg>\n'
    )


# --------------------------------------------------------------------------------------------- hero
COPY = {
    "en": {
        "tag": ("Source control for a world", "where agents write code too."),
        "sub": ("Crash-safe integration  ·  validation bound to the exact tree",
                "policy-bound agents  ·  a tamper-evident audit trail"),
        "verdicts": "FOUR INDEPENDENT VERDICTS PER CHANGE",
        "states": ("clean merge", "1/1 passed", "1 of 2", "not yet"),
        "foot": "Nothing implies another. Each is decided by its own evidence.",
        "desc": "Source control for a world where agents write code too: four independent verdicts per change.",
    },
    "ru": {
        "tag": ("Система контроля версий для мира,", "где код пишут и агенты."),
        "sub": ("Безопасная при сбоях интеграция  ·  валидация точного дерева",
                "агенты в рамках политики  ·  аудит, где видна подмена"),
        "verdicts": "ЧЕТЫРЕ НЕЗАВИСИМЫХ ВЕРДИКТА НА ИЗМЕНЕНИЕ",
        "states": ("чистое слияние", "1/1 пройдено", "1 из 2", "ещё нет"),
        "foot": "Ни один не подразумевает другой.",
        "desc": "Система контроля версий для мира, где код пишут и агенты: четыре независимых вердикта на изменение.",
    },
}


def hero(lang: str = "en") -> str:
    c = COPY[lang]
    W, H = 1280, 340
    b = []
    b.append(
        '<defs>'
        '<linearGradient id="bg" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#0a0f24"/>'
        '<stop offset="0.55" stop-color="#141a44"/><stop offset="1" stop-color="#241a5c"/></linearGradient>'
        '<linearGradient id="logo" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#5b8cff"/>'
        '<stop offset="1" stop-color="#8a5cff"/></linearGradient>'
        '<radialGradient id="glow" cx="0.82" cy="0.2" r="0.6"><stop offset="0" stop-color="#6d6bff" stop-opacity="0.45"/>'
        '<stop offset="1" stop-color="#6d6bff" stop-opacity="0"/></radialGradient>'
        '<pattern id="dots" width="26" height="26" patternUnits="userSpaceOnUse"><circle cx="2" cy="2" r="1.2" fill="#ffffff" fill-opacity="0.06"/></pattern>'
        '</defs>'
    )
    b.append(rect(0, 0, W, H, "url(#bg)", rx=22))
    b.append(rect(0, 0, W, H, "url(#dots)", rx=22))
    b.append(rect(0, 0, W, H, "url(#glow)", rx=22))

    # logo mark: a commit graph with a branch and a merge
    lx, ly = 72, 78
    b.append(rect(lx, ly, 92, 92, "url(#logo)", rx=22))
    nodes = [(lx + 24, ly + 24), (lx + 24, ly + 68), (lx + 68, ly + 46)]
    b.append(f'<path d="M{nodes[0][0]} {nodes[0][1]} L{nodes[1][0]} {nodes[1][1]}" stroke="#fff" stroke-width="5" stroke-linecap="round"/>')
    b.append(
        f'<path d="M{nodes[0][0]} {nodes[0][1]} C{nodes[0][0] + 26} {nodes[0][1]} {nodes[2][0] - 26} {nodes[2][1]} {nodes[2][0]} {nodes[2][1]}" '
        f'fill="none" stroke="#fff" stroke-width="5" stroke-linecap="round"/>'
    )
    b.append(
        f'<path d="M{nodes[1][0]} {nodes[1][1]} C{nodes[1][0] + 26} {nodes[1][1]} {nodes[2][0] - 26} {nodes[2][1]} {nodes[2][0]} {nodes[2][1]}" '
        f'fill="none" stroke="#fff" stroke-width="5" stroke-linecap="round"/>'
    )
    for x, y in nodes:
        b.append(f'<circle cx="{x}" cy="{y}" r="9" fill="#fff"/><circle cx="{x}" cy="{y}" r="4" fill="#5b6cff"/>')

    b.append(text(190, 138, "AgentGit", 70, "#ffffff", 800))
    b.append(text(74, 206, c["tag"][0], 33, "#e4e8ff", 700))
    b.append(text(74, 246, c["tag"][1], 33, "#e4e8ff", 700))
    b.append(text(74, 284, c["sub"][0], 17, "#9aa6e6", 400))
    b.append(text(74, 308, c["sub"][1], 17, "#9aa6e6", 400))

    # four independent verdicts, as in the UI
    px, py = 868, 70
    b.append(text(px, py - 12, c["verdicts"], 12, "#8e9bdc", 700, extra='letter-spacing="1.6"'))
    rows = [
        ("Mergeable", c["states"][0], "#0f3d2e", "#1f7a5a", "#7ef0bd", "✓"),
        ("Validated", c["states"][1], "#0f3d2e", "#1f7a5a", "#7ef0bd", "✓"),
        ("Approved", c["states"][2], "#4a3508", "#9a6c12", "#ffd37a", "!"),
        ("Integrated", c["states"][3], "#12305a", "#2c63b3", "#8fc2ff", "…"),
    ]
    for i, (name, state, bgc, bd, fg, glyph) in enumerate(rows):
        y = py + i * 58
        b.append(rect(px, y, 340, 46, bgc, bd, rx=23))
        b.append(f'<circle cx="{px + 24}" cy="{y + 23}" r="12" fill="{bd}"/>')
        b.append(text(px + 24, y + 28, glyph, 15, "#ffffff", 800, "middle"))
        b.append(text(px + 48, y + 29, name, 17, fg, 700))
        b.append(text(px + 322, y + 29, state, 15, fg, 500, "end"))
    b.append(text(px, py + 4 * 58 + 8, c["foot"], 13, "#8e9bdc", 400))
    return svg_doc(W, H, "".join(b), "AgentGit", c["desc"])


# ---------------------------------------------------------------------------------------- architecture
def lay_chips(x, y, labels, max_w, fill, stroke, color, gap=8, h=28, size=13, mono=False):
    """Chips laid out left to right, wrapping at max_w. Returns (svg, bottom_y)."""
    out, cx, cy = [], x, y
    for lab in labels:
        cw = size * (0.62 if mono else 0.60)
        w = int(len(lab) * cw + 24)
        if cx + w > x + max_w and cx > x:
            cx, cy = x, cy + h + gap
        svg, _ = chip(cx, cy, lab, fill, stroke, color, size=size, h=h, mono=mono, pad=12)
        out.append(svg)
        cx += w + gap
    return "".join(out), cy + h


def architecture() -> str:
    W = 1240
    LEFT, BW = 28, 884  # the main column
    inner = BW - 36
    parts = []  # (svg) in paint order

    def band(y, title, fill, stroke, groups, pad_bottom=16):
        """Draws a titled band whose height follows its chip groups; returns the bottom y."""
        chips_svg, cy = [], y + 40
        for labels, style in groups:
            svg, bottom = lay_chips(LEFT + 18, cy, labels, inner, **style)
            chips_svg.append(svg)
            cy = bottom + 10
        h = cy - y - 10 + pad_bottom
        parts.append(rect(LEFT, y, BW, h, fill, stroke, rx=14))
        parts.append(text(LEFT + 18, y + 25, title, 12, "#57606a", 700, extra='letter-spacing="1.4"'))
        parts.extend(chips_svg)
        return y + h

    # --- clients (cards, not chips)
    y0 = 24
    parts.append(rect(LEFT, y0, BW, 104, "#f5f7ff", "#c7d2fe", rx=14))
    parts.append(text(LEFT + 18, y0 + 25, "CLIENTS", 12, "#57606a", 700, extra='letter-spacing="1.4"'))
    xs = LEFT + 18
    for label, sub, w in [
        ("ag CLI", "local + remote", 150),
        ("Web UI", "Vue 3 · live board", 170),
        ("SDKs", "TypeScript · Python", 170),
        ("Agent platform", "Go · scheduler, advisor", 200),
        ("Rust client", "ag-client (gRPC)", 148),
    ]:
        parts.append(rect(xs, y0 + 38, w, 56, "#ffffff", "#a5b4fc", rx=10))
        parts.append(text(xs + w / 2, y0 + 62, label, 15, "#1e1b4b", 700, "middle"))
        parts.append(text(xs + w / 2, y0 + 81, sub, 12, "#57606a", 400, "middle"))
        xs += w + 10
    y = y0 + 104

    parts.append(arrow(LEFT + BW / 2, y + 4, LEFT + BW / 2, y + 44, "#4f46e5"))
    parts.append(text(LEFT + BW / 2 + 14, y + 30, "AGWP v1  ·  gRPC + HTTP/JSON + SSE  ·  one port", 13, "#3730a3", 600))
    y += 48

    sv = dict(fill="#ffffff", stroke="#7dd3fc", color="#0c4a6e")
    server_bottom = band(
        y, "AG-SERVER", "#eef9ff", "#7dd3fc",
        [
            (["OIDC · scoped, expiring credentials", "RBAC per repo / stream / operation", "rate limits · quotas"], sv),
            (["idempotency keys", "SSE events + replay", "Prometheus metrics", "OpenTelemetry traces", "execution-plane job queue"], sv),
        ],
    )
    server_y = y
    y = server_bottom
    parts.append(arrow(LEFT + BW / 2, y + 4, LEFT + BW / 2, y + 34, "#475569"))
    parts.append(text(LEFT + BW / 2 + 14, y + 24, "in process", 12, "#57606a", 400))
    y += 38

    en = dict(fill="#ffffff", stroke="#86efac", color="#14532d")
    y = band(
        y, "ENGINE  ·  ag-integrate", "#effcf3", "#86efac",
        [
            (["verified publication", "integration by compare-and-swap", "policy + signed attestations", "approvals bound to the head"], en),
            (["rename-aware merge", "leases + fencing tokens", "tasks · provenance · prediction", "queue", "blame", "GC · verify · backup"], en),
        ],
    )
    y += 20
    co = dict(fill="#ffffff", stroke="#afb8c1", color="#24292f", mono=True, h=30)
    core_bottom = band(
        y, "DETERMINISTIC CORE  ·  no model, no clock, no randomness in any decision", "#f6f8fa", "#d0d7de",
        [(["types", "object", "changeset", "tree", "dag", "diff", "merge", "policy", "agent"], co)],
        pad_bottom=34,
    )
    parts.append(text(LEFT + 18, core_bottom - 14, "content-addressed · canonical bytes checked against an independent Python implementation", 12, "#57606a", 400))
    y = core_bottom
    parts.append(arrow(LEFT + BW / 2, y + 4, LEFT + BW / 2, y + 30, "#475569"))
    y += 34

    # --- storage: two boxes
    top = y
    parts.append(rect(LEFT, top, BW, 128, "#fff8ee", "#fdba74", rx=14))
    parts.append(text(LEFT + 18, top + 25, "STORAGE  ·  chosen per repository", 12, "#57606a", 700, extra='letter-spacing="1.4"'))
    for bx, title, labels in [(LEFT + 18, "Metadata", ["SQLite (WAL)", "PostgreSQL"]), (LEFT + 18 + 436, "Objects", ["filesystem", "S3-compatible bucket"])]:
        parts.append(rect(bx, top + 38, 412, 76, "#ffffff", "#fdba74", rx=10))
        parts.append(text(bx + 16, top + 62, title, 13, "#7c2d12", 700))
        svg, _ = lay_chips(bx + 16, top + 74, labels, 380, fill="#fff4e5", stroke="#fdba74", color="#7c2d12")
        parts.append(svg)
    bottom = top + 128
    H = bottom + 24

    # --- execution plane (a separate process; untrusted code)
    X, PW = 940, 272
    parts.append(rect(X, 24, PW, H - 48, "#fff5f5", "#fca5a5", rx=14, dash="7 6"))
    parts.append(text(X + 18, 49, "EXECUTION PLANE", 12, "#991b1b", 700, extra='letter-spacing="1.4"'))
    parts.append(text(X + 18, 69, "a separate process", 12, "#57606a", 400))
    parts.append(rect(X + 18, 92, 236, 128, "#ffffff", "#fca5a5", rx=10))
    parts.append(text(X + 36, 118, "ag-worker", 16, "#7f1d1d", 800))
    for i, line in enumerate(["claims a validation job", "fetches the exact candidate tree", "recomputes its id, hashes every blob", "runs the policy's command", "signs a pass only for exit 0"]):
        parts.append(text(X + 36, 140 + i * 17, line, 12, "#3f3f46", 400))
    parts.append(arrow(X + 136, 220, X + 136, 262, "#b91c1c"))
    parts.append(rect(X + 18, 264, 236, 172, "#ffffff", "#fca5a5", rx=10))
    parts.append(text(X + 36, 290, "ag-sandbox", 16, "#7f1d1d", 800))
    for i, line in enumerate(["rootless user / mount / PID / net", "namespaces, read-only root", "capabilities dropped, no_new_privs", "seccomp deny-list, hardened /proc", "CPU · memory · process · file limits", "or an OCI container"]):
        parts.append(text(X + 36, 314 + i * 18, line, 12, "#3f3f46", 400))
    parts.append(rect(X + 18, 454, 236, 94, "#fef2f2", "#fca5a5", rx=10, dash="4 4"))
    parts.append(text(X + 136, 482, "untrusted code runs here", 13, "#991b1b", 700, "middle"))
    parts.append(text(X + 136, 504, "shares the host kernel (not a VM)", 12, "#57606a", 400, "middle"))
    parts.append(text(X + 136, 524, "never sees the worker's key", 12, "#57606a", 400, "middle"))
    mid = server_y + 40
    parts.append(f'<path d="M{LEFT + BW} {mid} L{X} {mid}" stroke="#b91c1c" stroke-width="2" stroke-dasharray="5 4" fill="none"/>')
    parts.append(text(X + 136, 580, "talks to ag-server over AGWP:", 12, "#57606a", 400, "middle"))
    parts.append(text(X + 136, 599, "claim · heartbeat · complete", 12, "#57606a", 600, "middle"))
    parts.append(text(X + 136, 618, "with a signed attestation", 12, "#57606a", 400, "middle"))
    parts.append(rect(X + 18, 646, 236, 66, "#ffffff", "#fca5a5", rx=10))
    parts.append(text(X + 136, 672, "Whether an attestation counts", 12, "#3f3f46", 700, "middle"))
    parts.append(text(X + 136, 691, "is decided by the stream's policy,", 12, "#3f3f46", 400, "middle"))
    parts.append(text(X + 136, 706, "never by who delivered it.", 12, "#3f3f46", 400, "middle"))

    body = rect(0, 0, W, H, "#ffffff", "#d0d7de", rx=18) + "".join(parts)
    return svg_doc(W, H, body, "AgentGit architecture", "Clients talk AGWP to ag-server; the engine sits on a deterministic core and per-repository storage; untrusted checks run in a separate sandboxed worker.")


FILES = {"hero.svg": hero, "hero.ru.svg": lambda: hero("ru"), "architecture.svg": architecture}


def main() -> int:
    check = "--check" in sys.argv
    bad = []
    for name, fn in FILES.items():
        path = OUT / name
        content = fn()
        if check:
            if not path.exists() or path.read_text() != content:
                bad.append(name)
        else:
            path.write_text(content)
    if bad:
        print("out of date: " + ", ".join(bad) + " (run scripts/gen_readme_assets.py)", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
