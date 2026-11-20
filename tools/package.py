"""Package a compiled Kindle Dashboard binary using the Python standard library."""

import argparse
import hashlib
import stat
import struct
import tempfile
from pathlib import Path
from zipfile import ZIP_DEFLATED, ZipFile, ZipInfo


ROOT = Path(__file__).resolve().parent.parent
DEFAULT_BINARY = ROOT / "client/target/armv7-unknown-linux-musleabihf/release/kindle-dashboard"


def check_binary(data):
    if len(data) < 52 or data[:7] != b"\x7fELF\x01\x01\x01":
        raise ValueError("expected a 32-bit little-endian ARM ELF")
    machine = struct.unpack_from("<H", data, 18)[0]
    flags = struct.unpack_from("<I", data, 36)[0]
    if machine != 40 or not flags & 0x400:
        raise ValueError("expected an ARM hard-float executable")
    offset = struct.unpack_from("<I", data, 28)[0]
    entry_size, count = struct.unpack_from("<HH", data, 42)
    if offset < 52 or entry_size < 32 or not count or offset + entry_size * count > len(data):
        raise ValueError("invalid ELF program header table")
    segments = {struct.unpack_from("<I", data, offset + index * entry_size)[0] for index in range(count)}
    if 3 in segments or 1 not in segments:
        raise ValueError("expected a statically linked executable with a loadable segment")


def add(archive, name, data, executable=False):
    info = ZipInfo(name)
    info.create_system = 3
    info.external_attr = (stat.S_IFREG | (0o755 if executable else 0o644)) << 16
    archive.writestr(info, data, compress_type=ZIP_DEFLATED)


def package(binary, output_dir):
    data = binary.read_bytes()
    check_binary(data)
    device = ROOT / "device"
    licenses = {
        "LICENSE.txt": ROOT / "LICENSE",
        "FONT-LICENSE.txt": ROOT / "client/assets/OFL.txt",
        "NOTO-LICENSE.txt": ROOT / "client/assets/NotoSansSC-OFL.txt",
        "BACKEND-LICENSE-MIT.txt": ROOT / "client/vendor/slint-backend-kindle/LICENSE-MIT",
        "BACKEND-LICENSE-APACHE.txt": ROOT / "client/vendor/slint-backend-kindle/LICENSE-APACHE",
    }

    output_dir.mkdir(parents=True, exist_ok=True)
    output = output_dir / "kpw4-dashboard.zip"
    with tempfile.TemporaryDirectory(dir=output_dir) as staging:
        staged = Path(staging) / output.name
        with ZipFile(staged, "w") as archive:
            for script in sorted(device.rglob("*.sh")):
                name = script.relative_to(device).as_posix()
                add(archive, name, script.read_bytes().replace(b"\r\n", b"\n"), executable=True)
            add(archive, "kindle-dashboard/kindle-dashboard", data, executable=True)
            for name, path in licenses.items():
                add(archive, "kindle-dashboard/" + name, path.read_bytes().replace(b"\r\n", b"\n"))
        staged.replace(output)
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output-dir", type=Path, default=ROOT / "dist")
    args = parser.parse_args()
    try:
        output = package(args.binary, args.output_dir)
    except (OSError, ValueError) as error:
        parser.exit(1, f"Packaging failed: {error}\n")
    print(f"{hashlib.sha256(output.read_bytes()).hexdigest()}  {output}")


if __name__ == "__main__":
    main()
