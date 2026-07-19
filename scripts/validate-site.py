#!/usr/bin/env python3
"""Validate the dependency-free static marketing site before deployment."""

from __future__ import annotations

import json
import re
import sys
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import unquote, urlsplit


ROOT = Path(__file__).resolve().parents[1]
SITE = ROOT / "site"
PROJECT_PREFIX = "/LocalAI-Cowork/"
GOOGLE_SITE_VERIFICATION = "googled44b625d58bdfa4e.html"
PRIVATE_IPV4 = re.compile(
    r"\b(?:10(?:\.\d{1,3}){3}|192\.168(?:\.\d{1,3}){2}|172\.(?:1[6-9]|2\d|3[01])(?:\.\d{1,3}){2})\b"
)
TRACKER_MARKERS = (
    "google-analytics",
    "googletagmanager",
    "plausible.io",
    "posthog",
    "segment.com",
    "sentry.io",
    "hotjar",
    "mixpanel",
)


class PageParser(HTMLParser):
    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.title = ""
        self._in_title = False
        self.h1_count = 0
        self.description = ""
        self.canonical = ""
        self.references: list[tuple[str, str]] = []
        self.images: list[dict[str, str]] = []
        self.json_ld: list[str] = []
        self._json_ld_buffer: list[str] | None = None

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        values = {key.lower(): value or "" for key, value in attrs}
        if tag == "title":
            self._in_title = True
        elif tag == "h1":
            self.h1_count += 1
        elif tag == "meta" and values.get("name", "").lower() == "description":
            self.description = values.get("content", "").strip()
        elif tag == "link" and values.get("rel", "").lower() == "canonical":
            self.canonical = values.get("href", "").strip()
        elif tag == "script" and values.get("type", "").lower() == "application/ld+json":
            self._json_ld_buffer = []
        elif tag == "img":
            self.images.append(values)

        for attribute in ("href", "src"):
            value = values.get(attribute, "").strip()
            if value:
                self.references.append((attribute, value))

    def handle_endtag(self, tag: str) -> None:
        if tag == "title":
            self._in_title = False
        elif tag == "script" and self._json_ld_buffer is not None:
            self.json_ld.append("".join(self._json_ld_buffer).strip())
            self._json_ld_buffer = None

    def handle_data(self, data: str) -> None:
        if self._in_title:
            self.title += data
        if self._json_ld_buffer is not None:
            self._json_ld_buffer.append(data)


def local_target(page: Path, reference: str) -> Path | None:
    parts = urlsplit(reference)
    if parts.scheme or parts.netloc or reference.startswith(("mailto:", "tel:", "#")):
        return None
    path = unquote(parts.path)
    if not path:
        return None
    if path.startswith(PROJECT_PREFIX):
        path = path[len(PROJECT_PREFIX) :]
        return SITE / (path or "index.html")
    if path.startswith("/"):
        return None
    target = (page.parent / path).resolve()
    if target.is_dir() or path.endswith("/"):
        target /= "index.html"
    return target


def png_dimensions(path: Path) -> tuple[int, int] | None:
    data = path.read_bytes()[:24]
    if len(data) != 24 or data[:8] != b"\x89PNG\r\n\x1a\n" or data[12:16] != b"IHDR":
        return None
    return int.from_bytes(data[16:20], "big"), int.from_bytes(data[20:24], "big")


def validate_html(path: Path, errors: list[str]) -> None:
    text = path.read_text(encoding="utf-8")
    relative = path.relative_to(ROOT).as_posix()
    parser = PageParser()
    parser.feed(text)

    if not parser.title.strip():
        errors.append(f"{relative}: missing title")
    if parser.h1_count != 1:
        errors.append(f"{relative}: expected one h1, found {parser.h1_count}")
    if 'name="robots" content="noindex"' not in text:
        if not parser.description:
            errors.append(f"{relative}: missing meta description")
        if not parser.canonical.startswith("https://noshitcoding.github.io/LocalAI-Cowork/"):
            errors.append(f"{relative}: missing or unexpected canonical URL")

    lowered = text.lower()
    for marker in TRACKER_MARKERS:
        if marker in lowered:
            errors.append(f"{relative}: tracker marker is not allowed: {marker}")
    if PRIVATE_IPV4.search(text):
        errors.append(f"{relative}: contains a private IPv4 address")

    for _, reference in parser.references:
        target = local_target(path, reference)
        if target is not None and not target.exists():
            errors.append(f"{relative}: broken local reference {reference!r}")

    for image in parser.images:
        target = local_target(path, image.get("src", ""))
        if target is None or not target.exists() or target.suffix.lower() != ".png":
            continue
        dimensions = png_dimensions(target)
        try:
            declared = int(image.get("width", "")), int(image.get("height", ""))
        except ValueError:
            errors.append(f"{relative}: PNG image is missing numeric width and height: {image.get('src', '')!r}")
            continue
        if dimensions is None or declared[0] <= 0 or declared[1] <= 0:
            errors.append(f"{relative}: invalid PNG image metadata: {image.get('src', '')!r}")
            continue
        actual_ratio = dimensions[0] / dimensions[1]
        declared_ratio = declared[0] / declared[1]
        if abs(actual_ratio - declared_ratio) > 0.001:
            errors.append(f"{relative}: distorted PNG aspect ratio for {image.get('src', '')!r}")

    for payload in parser.json_ld:
        try:
            json.loads(payload)
        except json.JSONDecodeError as error:
            errors.append(f"{relative}: invalid JSON-LD: {error.msg}")


def main() -> int:
    errors: list[str] = []
    required = (
        SITE / "index.html",
        SITE / "de" / "index.html",
        SITE / "claude-cowork-alternative" / "index.html",
        SITE / "copilot-cowork-alternative" / "index.html",
        SITE / "open-source-ai-cowork" / "index.html",
        SITE / "local-ai-agent-windows" / "index.html",
        SITE / "ollama-ai-agent" / "index.html",
        SITE / "private-ai-workspace" / "index.html",
        SITE / "ai-coworker-desktop" / "index.html",
        SITE / "privacy.html",
        SITE / "de" / "datenschutz.html",
        SITE / "robots.txt",
        SITE / "sitemap.xml",
        SITE / GOOGLE_SITE_VERIFICATION,
        SITE / "assets" / "logo.png",
        SITE / "assets" / "app-preview.png",
        SITE / "assets" / "github-social-preview.png",
    )
    for path in required:
        if not path.exists():
            errors.append(f"missing required site file: {path.relative_to(ROOT).as_posix()}")

    site_logo = SITE / "assets" / "logo.png"
    app_logo = ROOT / "app" / "src-tauri" / "icons" / "128x128.png"
    if site_logo.exists() and app_logo.exists() and site_logo.read_bytes() != app_logo.read_bytes():
        errors.append("site/assets/logo.png: does not match the canonical app logo")

    social_preview = SITE / "assets" / "github-social-preview.png"
    if social_preview.exists() and png_dimensions(social_preview) != (1280, 640):
        errors.append("site/assets/github-social-preview.png: expected 1280x640 pixels")

    verification_file = SITE / GOOGLE_SITE_VERIFICATION
    expected_verification = f"google-site-verification: {GOOGLE_SITE_VERIFICATION}"
    if verification_file.exists() and verification_file.read_text(encoding="utf-8").strip() != expected_verification:
        errors.append(f"site/{GOOGLE_SITE_VERIFICATION}: unexpected verification content")

    html_pages = [path for path in sorted(SITE.rglob("*.html")) if path != verification_file]
    for path in html_pages:
        validate_html(path, errors)

    robots = (SITE / "robots.txt").read_text(encoding="utf-8")
    if "Sitemap: https://noshitcoding.github.io/LocalAI-Cowork/sitemap.xml" not in robots:
        errors.append("site/robots.txt: sitemap URL is missing")

    sitemap = (SITE / "sitemap.xml").read_text(encoding="utf-8")
    for url in (
        "https://noshitcoding.github.io/LocalAI-Cowork/",
        "https://noshitcoding.github.io/LocalAI-Cowork/de/",
        "https://noshitcoding.github.io/LocalAI-Cowork/claude-cowork-alternative/",
        "https://noshitcoding.github.io/LocalAI-Cowork/copilot-cowork-alternative/",
        "https://noshitcoding.github.io/LocalAI-Cowork/open-source-ai-cowork/",
        "https://noshitcoding.github.io/LocalAI-Cowork/local-ai-agent-windows/",
        "https://noshitcoding.github.io/LocalAI-Cowork/ollama-ai-agent/",
        "https://noshitcoding.github.io/LocalAI-Cowork/private-ai-workspace/",
        "https://noshitcoding.github.io/LocalAI-Cowork/ai-coworker-desktop/",
        "https://noshitcoding.github.io/LocalAI-Cowork/privacy.html",
    ):
        if url not in sitemap:
            errors.append(f"site/sitemap.xml: missing {url}")

    if errors:
        print("Website validation failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1

    print(f"Website validation passed ({len(html_pages)} HTML pages plus Google verification).")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
