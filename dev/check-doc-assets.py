#!/usr/bin/env python3
"""Check local inline images in READMEs and assets in the static site's HTML/CSS."""

from html.parser import HTMLParser
from pathlib import Path
import re
import sys
from urllib.parse import unquote, urlsplit


class Assets(HTMLParser):
    def __init__(self):
        super().__init__()
        self.urls = []

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if tag in {"img", "script", "source"} and attrs.get("src"):
            self.urls.append(attrs["src"])
        if tag == "link" and attrs.get("href"):
            self.urls.append(attrs["href"])


def references(text, suffix):
    if suffix == ".css":
        return [value.strip(" \t\n\"'") for value in re.findall(r"url\(([^)]+)\)", text)]
    parser = Assets()
    parser.feed(text)
    if suffix == ".md":
        parser.urls.extend(match[0] or match[1] for match in re.findall(r"!\[[^\]]*\]\(\s*(?:<([^>]+)>|([^\s)]+))[^)]*\)", text))
    return parser.urls


def local_asset(source, root, url):
    parsed = urlsplit(url)
    if parsed.scheme or parsed.netloc or not parsed.path:
        return None
    path = unquote(parsed.path)
    return (root / path.lstrip("/") if path.startswith("/") else source.parent / path).resolve()


def main():
    if sys.argv[1:] == ["--self-test"]:
        assert references('![shot](assets/shot.png "Title") <img src="logo.png">', ".md") == ["logo.png", "assets/shot.png"]
        assert references('![shot](<assets/shot 1.png>)', ".md") == ["assets/shot 1.png"]
        assert references('<link href="/assets/site.css" rel="stylesheet">', ".html") == ["/assets/site.css"]
        assert references('a { background: url("../logo.png") }', ".css") == ["../logo.png"]
        root = Path("/repo/site")
        assert local_asset(root / "index.html", root, "/assets/logo%20one.png?v=1#x") == root / "assets/logo one.png"
        assert local_asset(root / "assets/site.css", root, "../logo.png") == root / "logo.png"
        for remote in ("https://example.com/image.png", "//example.com/image.png", "data:image/png,x", "#icon"):
            assert local_asset(root / "index.html", root, remote) is None
        print("Documentation asset checks: self-test passed")
        return
    root = Path(__file__).resolve().parent.parent
    readmes = [root / path for path in ("README.md", "README.zh-CN.md", "apps/macos/README.md", "crates/server/README.md")]
    site = root / "site"
    sources = readmes + sorted(site.rglob("*.html")) + sorted(site.rglob("*.css"))
    missing = []
    for source in sources:
        for url in references(source.read_text(encoding="utf-8"), source.suffix):
            asset = local_asset(source, root if source in readmes else site, url)
            if asset is not None and not asset.is_file():
                missing.append(f"{source.relative_to(root)}: missing asset {url}")
    if missing:
        sys.exit("\n".join(missing))
    print(f"Documentation assets exist in all {len(sources)} checked sources")


if __name__ == "__main__":
    main()
