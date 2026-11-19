"""Publish and serve a dashboard on a trusted LAN. Requires only Python 3."""
import argparse
import hashlib
import hmac
import ipaddress
import json
import os
import re
import struct
import tempfile
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parent
MAX_MANIFEST = 64 * 1024
MAX_ASSET = 192 * 1024
MAX_NODES = 80
ID = re.compile(r"[a-zA-Z0-9_-]{1,40}")
ASSET = re.compile(r"([0-9a-f]{64})\.(png|jpg)")
KEY = re.compile(r"[A-Za-z0-9._~-]{1,128}")


def require(condition, message):
    if not condition:
        raise ValueError(message)


def named(value):
    return isinstance(value, str) and ID.fullmatch(value) is not None


def bounded(value, low, high):
    return type(value) in (int, float) and low <= value <= high


def text(value, maximum):
    return isinstance(value, str) and 0 < len(value) <= maximum


def atomic_write(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as handle:
        temporary = Path(handle.name)
        try:
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
        except BaseException:
            handle.close()
            temporary.unlink(missing_ok=True)
            raise
    try:
        temporary.replace(path)
    finally:
        temporary.unlink(missing_ok=True)


def image_size(data, extension):
    if extension == "png":
        require(data[:16] == b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR" and len(data) >= 33,
                "invalid PNG header")
        return struct.unpack_from(">II", data, 16)
    require(data.startswith(b"\xff\xd8"), "invalid JPEG header")
    position = 2
    while position + 4 <= len(data) and data[position] == 0xFF:
        while position < len(data) and data[position] == 0xFF:
            position += 1
        if position + 3 > len(data):
            break
        marker = data[position]
        length = int.from_bytes(data[position + 1:position + 3], "big")
        if length < 2 or position + 1 + length > len(data):
            break
        if marker in (0xC0, 0xC1, 0xC2) and length >= 8:
            height, width = struct.unpack_from(">HH", data, position + 4)
            return width, height
        position += 1 + length
    raise ValueError("unsupported JPEG frame")


def manifest_for(draft, assets):
    source = draft.read_bytes()
    require(len(source) <= MAX_MANIFEST, "draft exceeds 64 KiB")
    manifest = json.loads(source)
    require(isinstance(manifest, dict) and type(manifest.get("version")) is int
            and manifest["version"] == 1, "expected protocol version 1")
    pages = manifest.get("pages")
    require(isinstance(pages, list) and 1 <= len(pages) <= 6, "expected 1–6 pages")
    page_ids = {page.get("id") for page in pages if isinstance(page, dict) and named(page.get("id"))}
    require(len(page_ids) == len(pages), "page IDs must be valid and distinct")
    require(named(manifest.get("defaultPage")) and manifest["defaultPage"] in page_ids,
            "defaultPage must identify a page")
    published_assets = {}
    resolved_sources = {}
    total = 0

    def validate(node, depth, buttons):
        nonlocal total
        total += 1
        require(depth <= 6 and total <= MAX_NODES, "too many blocks or nested containers")
        require(isinstance(node, dict), "block must be an object")
        kind = node.get("type")
        if kind in ("row", "column"):
            gap = node.get("gap", 16)
            require(type(gap) is int and 0 <= gap <= 48, "gap must be an integer from 0 to 48")
            children = node.get("children")
            require(isinstance(children, list) and 1 <= len(children) <= (4 if kind == "row" else 12),
                    "expected 1–4 children per row or 1–12 per column")
            if "weights" in node:
                weights = node["weights"]
                require(kind == "row" and isinstance(weights, list) and len(weights) == len(children)
                        and all(type(weight) is int and 1 <= weight <= 10 for weight in weights),
                        "row weights must match children and be integers from 1 to 10")
            for child in children:
                validate(child, depth + 1, buttons)
        elif kind == "text":
            require(text(node.get("text"), 600), "text must contain 1–600 characters")
            require(node.get("role", "body") in ("title", "heading", "value", "body", "caption"),
                    "invalid text role")
            require(node.get("align", "left") in ("left", "center", "right"), "invalid text alignment")
            if "maxLines" in node:
                lines = node["maxLines"]
                require(type(lines) is int and 1 <= lines <= 6, "maxLines must be 1–6")
        elif kind == "image":
            require(bounded(node.get("aspect", 1), 0.5, 4), "image aspect must be 0.5–4")
            require(node.get("fit", "contain") in ("contain", "cover"), "invalid image fit")
            require(text(node.get("alt"), 100), "image alt must contain 1–100 characters")
            name = node.get("src")
            require(isinstance(name, str) and re.fullmatch(r"[a-zA-Z0-9_-]{1,80}\.(png|jpg|jpeg)", name),
                    "draft image src must be a PNG or JPEG filename in the assets directory")
            if name not in resolved_sources:
                path = (assets / name).resolve()
                require(path.parent == assets, "image must stay within assets directory")
                with path.open("rb") as handle:
                    data = handle.read(MAX_ASSET + 1)
                require(len(data) <= MAX_ASSET, "image exceeds 192 KiB")
                extension = "png" if name.endswith(".png") else "jpg"
                width, height = image_size(data, extension)
                require(0 < width <= 800 and 0 < height <= 800, "image dimensions exceed 800×800")
                key = f"{hashlib.sha256(data).hexdigest()}.{extension}"
                published_assets[key] = data
                require(len(published_assets) <= 6, "at most 6 unique images per dashboard")
                resolved_sources[name] = "/assets/" + key
            node["src"] = resolved_sources[name]
        elif kind == "button":
            identity = node.get("id")
            require(named(identity) and identity not in buttons, "button IDs must be unique per page")
            buttons.add(identity)
            require(text(node.get("label"), 30), "button label must contain 1–30 characters")
            require(node.get("style", "secondary") in ("primary", "secondary"), "invalid button style")
            require(node.get("size", "normal") in ("normal", "large"), "invalid button size")
            action = node.get("action")
            require(isinstance(action, dict) and action.get("type") in ("refresh", "openPage"),
                    "unsupported button action")
            if action["type"] == "openPage":
                require(named(action.get("page")) and action["page"] in page_ids,
                        "openPage must identify an existing page")
        else:
            raise ValueError(f"unknown block type: {kind!r}")

    for page in pages:
        validate(page.get("body"), 1, set())
    payload = json.dumps(
        manifest, ensure_ascii=False, sort_keys=True, separators=(",", ":"), allow_nan=False
    ).encode("utf8")
    require(len(payload) <= MAX_MANIFEST, "published manifest exceeds 64 KiB")
    return payload, published_assets


def publish(draft, assets, directory):
    payload, image_files = manifest_for(draft, assets)
    for name, data in image_files.items():
        atomic_write(directory / "assets" / name, data)
    atomic_write(directory / "dashboard.json", payload)
    return len(payload), len(image_files)


def read_key(file, require_key):
    if file is not None:
        with file.open("r", encoding="utf8") as handle:
            raw = handle.read(512)
    else:
        raw = os.environ.get("DASHBOARD_API_KEY")
    if raw is None:
        require(not require_key, "set DASHBOARD_API_KEY or provide --key-file")
        return None
    value = raw.strip()
    require(len(raw) < 512 and KEY.fullmatch(value) is not None,
            "API key must have 1–128 letters, digits or . _ ~ -")
    return value.encode("ascii")


def handler(directory, key=None):
    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            path = urlsplit(self.path).path
            asset = ASSET.fullmatch(path[8:]) if path.startswith("/assets/") else None
            if path != "/api/v1/dashboard" and asset is None:
                self.send_error(404)
                return
            if key is not None and not self.authorized():
                self.send_response(401)
                self.send_header("WWW-Authenticate", 'Bearer realm="dashboard"')
                self.send_header("Cache-Control", "no-store")
                self.send_header("Vary", "Authorization")
                self.send_header("Content-Length", "0")
                self.end_headers()
                return
            if path == "/api/v1/dashboard":
                try:
                    payload = (directory / "dashboard.json").read_bytes()
                except FileNotFoundError:
                    self.send_error(503, "Publish a dashboard first")
                    return
                etag = '"' + hashlib.sha256(payload).hexdigest() + '"'
                if self.headers.get("If-None-Match") == etag:
                    self.send_response(304)
                    self.send_header("ETag", etag)
                    self.cache_headers("no-cache")
                    self.end_headers()
                    return
                self.send_data(payload, "application/json; charset=utf-8", etag)
            elif asset is not None:
                try:
                    payload = (directory / "assets" / asset[0]).read_bytes()
                except FileNotFoundError:
                    self.send_error(404)
                    return
                self.send_data(payload, "image/png" if asset[2] == "png" else "image/jpeg")

        def authorized(self):
            headers = self.headers.get_all("Authorization", [])
            if len(headers) != 1:
                return False
            scheme, _, token = headers[0].partition(" ")
            return scheme.lower() == "bearer" and hmac.compare_digest(token.encode("latin-1"), key)

        def cache_headers(self, directive):
            if key is not None:
                self.send_header("Cache-Control", "private, " + directive)
                self.send_header("Vary", "Authorization")
            else:
                self.send_header("Cache-Control", directive)

        def send_data(self, payload, content_type, etag=None):
            self.send_response(200)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(payload)))
            if etag:
                self.send_header("ETag", etag)
                self.cache_headers("no-cache")
            else:
                self.cache_headers("max-age=31536000, immutable")
            self.end_headers()
            self.wfile.write(payload)

    return Handler


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state", type=Path, default=ROOT / "published")
    actions = parser.add_subparsers(dest="action", required=True)
    publisher = actions.add_parser("publish")
    publisher.add_argument("--draft", type=Path, default=ROOT / "draft.json")
    publisher.add_argument("--assets", type=Path, default=ROOT / "assets")
    serve = actions.add_parser("serve")
    serve.add_argument("--host", default="127.0.0.1")
    serve.add_argument("--port", type=int, default=8765)
    serve.add_argument("--key-file", type=Path, help="Read the optional Bearer key from a file")
    serve.add_argument("--require-key", action="store_true", help="Refuse to start without a key")
    args = parser.parse_args()
    try:
        if args.action == "publish":
            size, count = publish(args.draft, args.assets.resolve(), args.state.resolve())
            print(f"Published {size} bytes and {count} image(s) to {args.state}")
            return
        address = ipaddress.IPv4Address(args.host)
        require(address.is_loopback or any(address in ipaddress.IPv4Network(network)
                for network in ("10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16")),
                "bind to loopback or an explicit private IPv4 address")
        key = read_key(args.key_file, args.require_key)
        with ThreadingHTTPServer((str(address), args.port), handler(args.state.resolve(), key)) as server:
            mode = "Bearer authentication enabled" if key is not None else "no authentication"
            print(f"Dashboard listening on http://{args.host}:{args.port} ({mode})", flush=True)
            server.serve_forever()
    except (OSError, ValueError, RecursionError) as error:
        parser.exit(1, f"Dashboard: {error}\n")
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
