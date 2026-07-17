#!/usr/bin/env python3
# /// script
# requires-python = ">=3.10"
# dependencies = ["model2vec==0.8.2"]
# ///
"""Build the embedded Nerd Fonts SQLite and vector-search index."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import sqlite3
import struct
import tempfile
from urllib.request import Request, urlopen

from model2vec import StaticModel

SOURCE_URL = "https://raw.githubusercontent.com/ryanoasis/nerd-fonts/v3.4.0/glyphnames.json"
SOURCE_SHA256 = "e2d10d23f5bff0bd6f0676e9b01d9789fcdc656de7b498a2955c27716ea4439c"
SOURCE_VERSION = "3.4.0"
EXPECTED_ICONS = 10_764
DIMENSIONS = 256
MODEL_NAME = "minishlab/potion-base-8M"
MODEL_REVISION = "bf8b056651a2c21b8d2565580b8569da283cab23"
MODEL_DIR = Path(__file__).resolve().parents[1] / "assets" / "model"
MODEL_FILES = {
    "config.json": "2a6ac0e9aaa356a68a5688070db78fc3a464fefe85d2f06a1905ce3718687553",
    "tokenizer.json": "e67e803f624fb4d67dea1c730d06e1067e1b14d830e2c2202569e3ef0f70bb50",
    "model.safetensors": "f65d0f325faadc1e121c319e2faa41170d3fa07d8c89abd48ca5358d9a223de2",
}

CATEGORY_INFO = {
    "cod": ("VS Code Codicons", "vscode codicon editor"),
    "custom": ("Nerd Fonts Custom", "nerd custom"),
    "dev": ("Devicons", "devicon development technology"),
    "extra": ("Font Awesome Extension", "extension extra"),
    "fa": ("Font Awesome", "fontawesome awesome"),
    "fae": ("Font Awesome Extension", "fontawesome extension"),
    "iec": ("IEC Power Symbols", "power electrical"),
    "indent": ("Indentation", "indent whitespace"),
    "indentation": ("Indentation", "indent whitespace"),
    "linux": ("Font Logos", "linux distro distribution operating system"),
    "md": ("Material Design Icons", "material design mdi"),
    "oct": ("GitHub Octicons", "github octicon"),
    "pl": ("Powerline", "powerline prompt"),
    "ple": ("Powerline Extra", "powerline prompt extra"),
    "pom": ("Pomicons", "pomicon"),
    "seti": ("Seti UI", "seti filetype"),
    "weather": ("Weather Icons", "forecast climate"),
}

# Each group expands user intent; only its first, visual anchor expands icon metadata.
# A term may belong to multiple groups, connecting concepts such as hearts and stars.
SEMANTIC_GROUPS = (
    ("add", "create", "new", "plus", "insert", "append"),
    ("delete", "remove", "trash", "garbage", "bin", "discard", "erase"),
    ("subtract", "minus", "decrement"),
    ("close", "cancel", "dismiss", "times", "cross"),
    ("confirm", "check", "done", "success", "approve", "tick", "accept"),
    ("error", "failure", "fail", "broken", "invalid", "danger"),
    ("warning", "alert", "caution", "attention", "exclamation"),
    ("information", "info", "about", "details"),
    ("help", "question", "support", "unknown"),
    ("search", "find", "lookup", "magnify", "magnifier", "explore"),
    ("settings", "setting", "config", "configuration", "preferences", "gear", "cog", "tune"),
    ("home", "house", "homepage", "start"),
    ("user", "account", "person", "profile", "avatar", "human"),
    ("users", "people", "team", "group", "community", "contacts"),
    ("login", "signin", "enter", "authenticate"),
    ("logout", "signout", "exit", "leave"),
    ("lock", "secure", "security", "private", "protected"),
    ("unlock", "unsecure", "openlock"),
    ("key", "password", "credential", "secret", "authentication"),
    ("shield", "defense", "protection", "safety"),
    ("eye", "view", "visible", "show", "preview", "watch"),
    ("hidden", "hide", "invisible", "visibilityoff"),
    ("edit", "pencil", "modify", "write", "compose"),
    ("save", "floppy", "diskette", "persist"),
    ("copy", "duplicate", "clone"),
    ("paste", "clipboard"),
    ("print", "printer"),
    ("file", "document", "page", "paper"),
    ("folder", "directory", "archive", "collection"),
    ("database", "storage", "data", "sql", "datastore", "cylinder"),
    ("server", "hosting", "datacenter", "rack"),
    ("cloud", "remote", "online"),
    ("upload", "publish", "push"),
    ("download", "fetch", "pull"),
    ("sync", "synchronize", "refresh", "reload", "renew", "repeat"),
    ("code", "program", "programming", "developer", "development", "source", "brackets"),
    ("terminal", "console", "shell", "commandline", "cli", "prompt"),
    ("git", "repository", "repo", "versioncontrol", "scm"),
    ("branch", "fork", "diverge"),
    ("merge", "combine", "join"),
    ("commit", "revision", "changeset", "history"),
    ("bug", "debug", "issue", "defect", "insect"),
    ("test", "testing", "experiment", "flask", "beaker"),
    ("package", "parcel", "box", "bundle"),
    ("container", "docker", "shipping"),
    ("kubernetes", "k8s", "orchestration", "helm"),
    ("network", "connected", "connection", "topology"),
    ("wifi", "wireless", "signal"),
    ("ethernet", "wired", "lan"),
    ("globe", "world", "earth", "web", "internet", "planet"),
    ("link", "url", "chain", "hyperlink", "attachment"),
    ("unlink", "brokenlink", "detach"),
    ("mail", "email", "envelope", "inbox", "letter"),
    ("message", "chat", "comment", "conversation", "bubble", "feedback"),
    ("phone", "telephone", "call", "handset"),
    ("calendar", "date", "event", "schedule"),
    ("clock", "time", "timer", "watch", "recent", "history"),
    ("alarm", "reminder", "wakeup"),
    ("bell", "notification", "notify", "ring"),
    ("heart", "love", "like", "favorite"),
    ("star", "rating", "favorite", "featured"),
    ("bookmark", "remember", "saved"),
    ("tag", "label", "price"),
    ("cart", "shopping", "basket", "checkout", "purchase"),
    ("store", "shop", "market", "retail"),
    ("money", "cash", "currency", "payment", "finance", "bank", "dollar"),
    ("gift", "present", "reward"),
    ("play", "start", "resume", "triangle"),
    ("pause", "hold"),
    ("stop", "halt", "square"),
    ("next", "forward", "skip"),
    ("previous", "back", "backward", "rewind"),
    ("music", "song", "note", "melody", "audio"),
    ("volume", "speaker", "sound", "loud"),
    ("mute", "silent", "quiet", "volumeoff"),
    ("microphone", "mic", "record", "voice"),
    ("headphones", "headset", "listen"),
    ("image", "photo", "picture", "photograph", "gallery", "landscape"),
    ("camera", "photography", "snapshot"),
    ("video", "movie", "film", "cinema", "camcorder"),
    ("map", "location", "place", "geography"),
    ("pin", "marker", "location", "point"),
    ("compass", "navigation", "direction"),
    ("arrowup", "up", "north", "ascending", "increase"),
    ("arrowdown", "down", "south", "descending", "decrease"),
    ("arrowleft", "left", "west", "back"),
    ("arrowright", "right", "east", "forward"),
    ("expand", "maximize", "fullscreen", "enlarge", "grow"),
    ("collapse", "minimize", "shrink", "contract"),
    ("move", "drag", "reorder", "position"),
    ("sort", "order", "arrange"),
    ("filter", "funnel", "refine"),
    ("menu", "hamburger", "navigation", "more"),
    ("dashboard", "overview", "panel", "gauge"),
    ("chart", "analytics", "statistics", "graph", "metrics", "trend"),
    ("list", "rows", "items"),
    ("grid", "tiles", "table", "layout"),
    ("share", "send", "forward"),
    ("external", "launch", "open", "newwindow"),
    ("power", "shutdown", "switch", "energy"),
    ("lightning", "bolt", "electricity", "fast", "flash"),
    ("battery", "charge", "charging"),
    ("sun", "day", "light", "bright", "brightness"),
    ("moon", "night", "dark", "theme"),
    ("weather", "forecast", "climate"),
    ("rain", "rainy", "umbrella", "droplet", "water"),
    ("snow", "snowflake", "cold", "winter"),
    ("fire", "flame", "hot", "trending"),
    ("rocket", "launch", "startup", "space"),
    ("robot", "bot", "automation", "android"),
    ("brain", "intelligence", "smart", "ai", "machinelearning"),
    ("magic", "wand", "sparkles", "automatic"),
    ("desktop", "monitor", "screen", "display", "computer"),
    ("laptop", "notebook", "computer"),
    ("mobile", "smartphone", "device", "cellphone"),
    ("tablet", "ipad", "device"),
    ("keyboard", "typing", "input"),
    ("mouse", "cursor", "pointer", "click"),
    ("cpu", "processor", "chip", "compute"),
    ("memory", "ram", "hardware"),
    ("bluetooth", "wireless", "pair"),
    ("language", "translate", "translation", "locale"),
    ("accessible", "accessibility", "wheelchair"),
    ("face", "smile", "happy", "emoji", "emotion"),
    ("sad", "unhappy", "frown", "emotion"),
    ("food", "meal", "restaurant", "eat", "fork", "knife"),
    ("coffee", "drink", "cup", "cafe"),
    ("tree", "nature", "forest", "plant"),
    ("leaf", "nature", "eco", "green"),
    ("book", "read", "documentation", "library", "manual"),
    ("graduation", "education", "school", "learn", "student"),
    ("building", "office", "business", "company"),
    ("tools", "wrench", "hammer", "repair", "maintenance"),
)

TOKEN_RE = re.compile(r"[a-z0-9]+")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, help="use a local glyphnames.json")
    parser.add_argument("--output", type=Path, default=Path("assets/icons.db"))
    return parser.parse_args()


def source_bytes(path: Path | None) -> bytes:
    if path is not None:
        data = path.read_bytes()
    else:
        request = Request(SOURCE_URL, headers={"User-Agent": "findnerd-index-builder"})
        with urlopen(request, timeout=30) as response:
            data = response.read()
    actual = hashlib.sha256(data).hexdigest()
    if actual != SOURCE_SHA256:
        raise SystemExit(f"glyph catalog checksum mismatch: expected {SOURCE_SHA256}, got {actual}")
    return data


def tokens(text: str) -> list[str]:
    return TOKEN_RE.findall(text.lower())


def semantic_maps() -> tuple[dict[str, str], dict[str, tuple[str, ...]]]:
    expansions: dict[str, set[str]] = {}
    anchors: dict[str, tuple[str, ...]] = {}
    for group in SEMANTIC_GROUPS:
        anchors[group[0]] = group
        for term in group:
            if len(tokens(term)) != 1:
                raise ValueError(f"semantic term must be one token: {term}")
            expansions.setdefault(term, set()).update(group)
    return (
        {term: " ".join(sorted(related)) for term, related in expansions.items()},
        anchors,
    )


def load_model() -> StaticModel:
    for name, expected in MODEL_FILES.items():
        path = MODEL_DIR / name
        actual = hashlib.sha256(path.read_bytes()).hexdigest()
        if actual != expected:
            raise SystemExit(f"model checksum mismatch for {name}: expected {expected}, got {actual}")
    return StaticModel.from_pretrained(str(MODEL_DIR))


def quantize(vector: object) -> bytes:
    values = [float(value) for value in vector]
    length = math.sqrt(sum(value * value for value in values))
    if length == 0:
        raise ValueError("embedding cannot be empty")
    quantized = [max(-127, min(127, round(value / length * 127))) for value in values]
    if len(quantized) != DIMENSIONS:
        raise ValueError(f"expected {DIMENSIONS} embedding dimensions, found {len(quantized)}")
    return struct.pack(f"{DIMENSIONS}b", *quantized)


def build_database(raw: bytes, output: Path) -> None:
    document = json.loads(raw)
    metadata = document.pop("METADATA")
    records = sorted(document.items())
    if len(records) != EXPECTED_ICONS:
        raise SystemExit(f"expected {EXPECTED_ICONS} icons, found {len(records)}")

    expansions, anchor_groups = semantic_maps()
    rows: list[tuple[str, str, str, int, str, str, str]] = []
    semantic_texts: list[str] = []
    category_counts: dict[str, int] = {}

    for name, glyph_data in records:
        category, separator, raw_label = name.partition("-")
        if not separator or category not in CATEGORY_INFO:
            raise ValueError(f"unknown icon category in {name}")
        label = raw_label.replace("_", " ").replace("-", " ")
        label_tokens = tokens(label)

        aliases: set[str] = set()
        for token in label_tokens:
            if group := anchor_groups.get(token):
                aliases.update(group)
        aliases.difference_update(label_tokens)
        alias_text = " ".join(sorted(aliases))

        category_name, category_aliases = CATEGORY_INFO[category]
        codepoint = int(glyph_data["code"], 16)
        search_text = " ".join(
            (name.replace("-", " ").replace("_", " "), label, alias_text, category_name, category_aliases)
        )

        rows.append(
            (
                name,
                label,
                glyph_data["char"],
                codepoint,
                category,
                alias_text,
                search_text,
            )
        )
        semantic_texts.append(" ".join((label, alias_text)).strip())
        category_counts[category] = category_counts.get(category, 0) + 1

    model = load_model()
    embeddings = model.encode(semantic_texts)
    if len(embeddings) != len(rows):
        raise ValueError(f"model returned {len(embeddings)} vectors for {len(rows)} icons")
    prepared = [row + (quantize(embedding),) for row, embedding in zip(rows, embeddings, strict=True)]

    output.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(prefix="findnerd-", suffix=".db", dir=output.parent)
    os.close(descriptor)
    temporary = Path(temporary_name)
    try:
        connection = sqlite3.connect(temporary)
        connection.executescript(
            f"""
            PRAGMA page_size = 4096;
            PRAGMA journal_mode = OFF;
            PRAGMA synchronous = OFF;
            PRAGMA application_id = 0x464E5244;
            PRAGMA user_version = 2;

            CREATE TABLE meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            ) STRICT;

            CREATE TABLE category (
                slug TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                aliases TEXT NOT NULL,
                icon_count INTEGER NOT NULL
            ) STRICT;

            CREATE TABLE icon (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                label TEXT NOT NULL,
                glyph TEXT NOT NULL,
                codepoint INTEGER NOT NULL,
                category TEXT NOT NULL REFERENCES category(slug),
                aliases TEXT NOT NULL,
                search_text TEXT NOT NULL,
                embedding BLOB NOT NULL CHECK(length(embedding) = {DIMENSIONS})
            ) STRICT;

            CREATE INDEX icon_category ON icon(category);

            CREATE TABLE query_expansion (
                term TEXT PRIMARY KEY,
                expansion TEXT NOT NULL
            ) STRICT;

            CREATE VIRTUAL TABLE icon_fts USING fts5(
                name,
                label,
                aliases,
                search_text,
                content='icon',
                content_rowid='id',
                tokenize="unicode61 remove_diacritics 2 tokenchars '_-'")
            ;
            """
        )
        connection.executemany(
            "INSERT INTO meta(key, value) VALUES (?, ?)",
            (
                ("schema_version", "2"),
                ("dimensions", str(DIMENSIONS)),
                ("embedding_model", MODEL_NAME),
                ("embedding_revision", MODEL_REVISION),
                ("source", SOURCE_URL),
                ("source_sha256", SOURCE_SHA256),
                ("source_version", SOURCE_VERSION),
                ("source_date", metadata["date"]),
            ),
        )
        connection.executemany(
            "INSERT INTO category(slug, name, aliases, icon_count) VALUES (?, ?, ?, ?)",
            (
                (slug, name, aliases, category_counts.get(slug, 0))
                for slug, (name, aliases) in CATEGORY_INFO.items()
            ),
        )
        connection.executemany(
            """INSERT INTO icon(name, label, glyph, codepoint, category, aliases, search_text, embedding)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?)""",
            prepared,
        )
        connection.executemany(
            "INSERT INTO query_expansion(term, expansion) VALUES (?, ?)",
            sorted(expansions.items()),
        )
        connection.execute("INSERT INTO icon_fts(icon_fts) VALUES ('rebuild')")
        connection.execute("INSERT INTO icon_fts(icon_fts, rank) VALUES ('rank', 'bm25(8.0, 5.0, 2.0, 0.5)')")
        connection.commit()
        connection.execute("VACUUM")
        connection.execute("PRAGMA optimize")
        connection.close()
        os.replace(temporary, output)
    finally:
        temporary.unlink(missing_ok=True)

    digest = hashlib.sha256(output.read_bytes()).hexdigest()
    print(f"wrote {output} ({len(prepared)} icons, sha256 {digest})")


def main() -> None:
    arguments = parse_args()
    build_database(source_bytes(arguments.source), arguments.output)


if __name__ == "__main__":
    main()
