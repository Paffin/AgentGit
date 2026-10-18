"""MkDocs hooks for the AgentGit documentation site (see mkdocs.yml, `hooks:`).

`docs/` stays the source of truth and keeps working on GitHub, where a link such as `../ROADMAP.md` or
`../../crates/ag-sandbox/src/spec.rs` is a plain relative link. The site only contains `docs/`, so two things need
help, and nothing else is done here:

1. Links that leave `docs/` (to the root README, ROADMAP, source files, workflows, ...) are rewritten to the
   repository on GitHub, at `extra.source_ref` (default `main`; CI sets `DOCS_SOURCE_REF` to the commit that is
   published, so a deployed page links to the exact files it was built from). A link to a file that does not exist
   is *not* rewritten: MkDocs then reports it and the strict build fails, which is what we want.
2. Pages that exist in `docs/` but are missing from `nav` (other work packages add pages) are appended to the
   navigation under "More", so a new page is never silently unreachable. The build prints them; keep `nav` current.
"""
from __future__ import annotations

import logging
import os
import posixpath
import re
from pathlib import Path

log = logging.getLogger("mkdocs.hooks.agentgit")

_LINK = re.compile(r"(?P<head>!?\[[^\]]*\]\()(?P<target>[^)\s]+)(?P<tail>(?:\s+\"[^\"]*\")?\))")
_FENCE = re.compile(r"^(```|~~~)")
_SCHEME = re.compile(r"^(?:[a-zA-Z][a-zA-Z0-9+.-]*:|//|#)")
_appended: list[dict] = []


def _repo_root(config) -> Path:
    return Path(config["config_file_path"]).resolve().parent


def _source_ref(config) -> str:
    return os.environ.get("DOCS_SOURCE_REF") or (config.get("extra") or {}).get("source_ref") or "main"


def _outside_docs(page_src: str, target: str, config) -> str | None:
    """The repository-relative path `target` points at, if it leaves docs_dir and exists; else None."""
    root = _repo_root(config)
    docs = Path(config["docs_dir"]).resolve()
    path, _, _ = target.partition("#")
    path = path.split("?")[0]
    if not path or _SCHEME.match(path):
        return None
    here = (docs / page_src).parent
    resolved = (here / path).resolve()
    try:
        resolved.relative_to(docs)
        return None  # inside docs: MkDocs resolves (and validates) it
    except ValueError:
        pass
    try:
        rel = resolved.relative_to(root)
    except ValueError:
        return None
    return rel.as_posix() if resolved.exists() else None


def _directory_without_index(page_src: str, target: str, config) -> str | None:
    """A link to a directory *inside* docs with no index page (`adr/`): the docs path to browse on GitHub."""
    docs = Path(config["docs_dir"]).resolve()
    path, _, _ = target.partition("#")
    if not path or _SCHEME.match(path):
        return None
    resolved = ((docs / page_src).parent / path).resolve()
    try:
        rel = resolved.relative_to(docs)
    except ValueError:
        return None
    if resolved.is_dir() and not any((resolved / n).exists() for n in ("README.md", "index.md")):
        return (Path(config["docs_dir"]).resolve().relative_to(_repo_root(config)) / rel).as_posix()
    return None


def on_page_markdown(markdown: str, page, config, files):
    repo = config["repo_url"].rstrip("/")
    ref = _source_ref(config)
    out: list[str] = []
    in_fence = False
    for line in markdown.split("\n"):
        if _FENCE.match(line.lstrip()):
            in_fence = not in_fence
        if in_fence:
            out.append(line)
            continue

        def rewrite(m: re.Match) -> str:
            target = m.group("target")
            repo_path = _outside_docs(page.file.src_path, target, config) or _directory_without_index(
                page.file.src_path, target, config
            )
            if repo_path is None:
                return m.group(0)
            full = Path(_repo_root(config), repo_path)
            kind = "tree" if full.is_dir() else "blob"
            frag = target.partition("#")[2]
            url = f"{repo}/{kind}/{ref}/{posixpath.normpath(repo_path)}" + (f"#{frag}" if frag else "")
            return f"{m.group('head')}{url}{m.group('tail')}"

        out.append(_LINK.sub(rewrite, line))
    return "\n".join(out)


def on_files(files, config):
    """Append pages that are not in `nav` to a "More" section (only when `nav` is given)."""
    nav = config.get("nav")
    if not nav:
        return files

    listed: set[str] = set()

    def walk(items) -> None:
        for item in items:
            if isinstance(item, dict):
                for v in item.values():
                    walk([v] if isinstance(v, (str, list)) else [])
            elif isinstance(item, list):
                walk(item)
            elif isinstance(item, str):
                listed.add(item)

    # `mkdocs serve` calls this on every rebuild with the same config: drop what an earlier call appended.
    if _appended and _appended[0] in nav:
        nav.remove(_appended[0])
        _appended.clear()
    walk(nav)
    extra = sorted(f.src_uri for f in files.documentation_pages() if f.src_uri not in listed)
    if extra:
        log.info("pages missing from nav, appended under 'More': %s", ", ".join(extra))
        entry = {"More": extra}
        nav.append(entry)
        _appended.append(entry)
    return files
