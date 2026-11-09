#!/usr/bin/env python3
"""Offline build of pinned liberofs plus the narrow vfile input patch.

This does not invoke mkfs.erofs and does not encode tar. It builds the actual
upstream C library. --source is always a local complete source checkout/export.
The original upstream directory is never modified. Logs and stamp live under
--out. A missing dependency is a failure, not a passing/skipped native test.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

HERE = Path(__file__).resolve().parent
LOCK = json.loads((HERE / "upstream.lock.json").read_text())


# Also works when loaded by a diagnostic runner via importlib.
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))
from source_store import validate_source as validate_tree


def validate_source(source: Path) -> None:
    validate_tree(source, LOCK)


def artifact_hashes(library: Path, includes: list[str]) -> dict[str, str]:
    """Authenticate both the archive and the exact headers used by the shim."""
    paths = {library}
    for inc in includes:
        directory = Path(inc)
        # include/ and lib/ are source headers; build/ contributes config.h only.
        if directory.name == "build":
            paths.add(directory / "config.h")
        else:
            paths.update(directory.rglob("*.h"))
    return {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(paths)}


def cache_intact(record: dict) -> bool:
    expected = record.get("artifacts")
    if not isinstance(expected, dict) or not expected:
        return False
    try:
        return artifact_hashes(Path(record["library"]), record["includes"]) == expected
    except (OSError, KeyError, TypeError):
        return False


def replace_once(path: Path, old: str, new: str):
    value = path.read_text()
    count = value.count(old)
    if count != 1:
        raise RuntimeError(f"patch anchor mismatch: {path.name}: {count} occurrences")
    path.write_text(value.replace(old, new, 1))


def patch_source(root: Path):
    inode = root / "lib/inode.c"
    # Hybrid xattr listing reached NULL infix with length zero. C still forbids
    # memcpy(NULL, ..., 0); keep the original vendor bytes unchanged.
    replace_once(root / "lib/xattr.c",
        "\tmemcpy(it->buffer + it->buffer_ofs + prefix_len, infix, infix_len);",
        "\tif (infix_len)\n\t\tmemcpy(it->buffer + it->buffer_ofs + prefix_len, infix, infix_len);")
    # Upstream snapshot has two empty-device loops doing NULL + 0. UBSan
    # catches these in ordinary standalone images. Guard before pointer math;
    # this preserves the nonempty-device behavior and changes no disk format.
    for function in ("erofs_update_all_devices", "erofs_flush_all_devices"):
        super_c = root / "lib/super.c"
        value = super_c.read_text()
        start = value.index("int " + function + "(")
        end = value.find("\n}\n", start) + len("\n}\n")
        body = value[start:end]
        old = "for (di = sbi->devs; di < sbi->devs + sbi->extra_devices; ++di)"
        if body.count(old) != 1:
            raise RuntimeError("device-loop patch anchor mismatch: " + function)
        body = body.replace(old, "for (di = sbi->devs; sbi->extra_devices &&\n\t     di < sbi->devs + sbi->extra_devices; ++di)", 1)
        super_c.write_text(value[:start] + body + value[end:])
    # An abort must release buffers without padding the failed output. Otherwise
    # a latched I/O error makes __erofs_bflush stop early and leaks the remaining
    # buffer graph in a long-lived FFI process (observed with LSan).
    replace_once(root / "lib/cache.c", "\t\tif (padding != blksiz) {",
                 "\t\tif (!abort && padding != blksiz) {")
    replace_once(root / "include/erofs/internal.h", "\tchar *i_srcpath;\n", "\tchar *i_srcpath;\n\t/* Polar-owned, borrowed positional input. No format changes. */\n\tstruct erofs_vfile *pcfs_vf;\n")
    # Native raw-file projections use chunk inodes. Upstream only sets the
    # 16-bit format, leaving the adjacent reserved field uninitialized. This
    # leaked two stack bytes and made identical metadata nondeterministic under
    # clang/ASan. Initialize the entire union before any on-disk serialization.
    replace_once(inode, "union erofs_inode_i_u u1;", "union erofs_inode_i_u u1 = {0};")
    replace_once(inode, "#define S_SHIFT                 12", "static int pcfs_write_file(struct erofs_importer *, struct erofs_inode *);\n\n#define S_SHIFT                 12")
    replace_once(inode,
        "if (S_ISREG(inode->i_mode) && inode->i_size) {",
        "if (S_ISREG(inode->i_mode) && inode->i_size && !inode->pcfs_vf) {")
    replace_once(inode,
        "\t} else if (inode->i_size) {\n\t\tif (inode->datasource == EROFS_INODE_DATA_SOURCE_RESVSP)",
        "\t} else if (inode->i_size) {\n\t\tif (inode->pcfs_vf)\n\t\t\tret = pcfs_write_file(btctx->im, inode);\n\t\telse if (inode->datasource == EROFS_INODE_DATA_SOURCE_RESVSP)")
    replace_once(inode,
        "\terofs_prepare_inode_buffer(btctx->im, inode);\n\terofs_write_tail_end(btctx->im, inode);\n\treturn 0;",
        "\tret = erofs_prepare_inode_buffer(btctx->im, inode);\n\treturn ret ? ret : erofs_write_tail_end(btctx->im, inode);")
    with inode.open("a") as f:
        f.write('\n#include "pcfs_importer.inc"\n')
    shutil.copyfile(HERE / "upstream/pcfs_importer.inc", root / "lib/pcfs_importer.inc")
    # Export the upstream single-file metadata rebuild, not a new disk encoder.
    # The Polar adapter validates native-compatible layouts before this call.
    replace_once(root / "lib/rebuild.c", "map.m_la = i << chunkbits;",
                 "map.m_la = (erofs_off_t)i << chunkbits;")
    with (root / "lib/rebuild.c").open("a") as f:
        f.write("\nint pcfs_erofs_rebuild_file(struct erofs_sb_info *, struct erofs_inode *);\n"
                "int pcfs_erofs_rebuild_file(struct erofs_sb_info *dst, struct erofs_inode *inode)\n"
                "{\n\tif (!S_ISREG(inode->i_mode)) return -EINVAL;\n"
                "\treturn erofs_rebuild_update_inode(dst, inode, EROFS_REBUILD_DATA_BLOB_INDEX);\n}\n")
    header = root / "lib/liberofs_compress.h"
    replace_once(header,
        "int erofs_begin_compressed_file(struct z_erofs_compress_ictx *ictx);",
        "void erofs_bind_compressed_file_with_vfile(struct z_erofs_compress_ictx *,\n\t\t\t\t\t struct erofs_vfile *, u64);\nint erofs_begin_compressed_file(struct z_erofs_compress_ictx *ictx);")
    replace_once(root / "lib/compress.c",
        "int erofs_begin_compressed_file(struct z_erofs_compress_ictx *ictx)\n{",
        "void erofs_bind_compressed_file_with_vfile(struct z_erofs_compress_ictx *ictx,\n\t\t\t\t\t struct erofs_vfile *vf, u64 fpos)\n{\n\t/* Borrow the actual wrapper; copying it would lose its enclosing context. */\n\tictx->vf = vf;\n\tictx->fpos = fpos;\n}\n\nint erofs_begin_compressed_file(struct z_erofs_compress_ictx *ictx)\n{")


def build(source: Path, out: Path, sanitize: bool = False) -> dict:
    source = source.resolve(); out = out.resolve()
    if not source.is_dir():
        raise RuntimeError("complete erofs-utils source is missing; run native/fetch_erofs.py explicitly or supply --source")
    if out == source or out.is_relative_to(source) or source.is_relative_to(out):
        raise RuntimeError("source and output directories must not contain one another")
    validate_source(source)
    fingerprint = hashlib.sha256(
        (HERE / "upstream.lock.json").read_bytes() + Path(__file__).read_bytes() +
        (HERE / "source_store.py").read_bytes() +
        (HERE / "upstream/pcfs_importer.inc").read_bytes() +
        json.dumps({"sanitize": sanitize, "CC": os.environ.get("CC", "cc"),
                    "CFLAGS": os.environ.get("CFLAGS", ""),
                    "LDFLAGS": os.environ.get("LDFLAGS", "")}, sort_keys=True).encode()
    ).hexdigest()
    stamp = out / "native-build.json"
    if stamp.exists():
        old = json.loads(stamp.read_text())
        if old.get("fingerprint") == fingerprint and Path(old["library"]).is_file():
            if cache_intact(old):
                return old
            raise RuntimeError("cached native library/config/headers changed; preserve this output for diagnosis and choose a fresh --out")
        raise RuntimeError("existing build has different inputs; use a fresh --out")
    if out.exists() and any(out.iterdir()):
        raise RuntimeError("incomplete nonempty build directory; preserve its logs and choose a fresh --out")
    out.mkdir(parents=True, exist_ok=True)
    root = out / "source"
    shutil.copytree(source, root, ignore=shutil.ignore_patterns(".git", ".libs", "*.o", "*.lo", "*.la"))
    # Recheck the private copy before executing autoreconf/configure.
    validate_source(root)
    patch_source(root)
    log = out / "build.log"
    env = os.environ.copy()
    cflags = env.get("CFLAGS", "-O2") + " -fPIC -ffunction-sections -fdata-sections"
    if sanitize:
        cflags += " -O1 -g -fsanitize=address,undefined -fno-omit-frame-pointer"
        env["LDFLAGS"] = env.get("LDFLAGS", "") + " -fsanitize=address,undefined"
    env["CFLAGS"] = cflags
    def run(argv, cwd):
        with log.open("a") as f:
            f.write("$ " + " ".join(map(str, argv)) + "\n"); f.flush()
            subprocess.run(list(map(str, argv)), cwd=cwd, env=env, stdout=f,
                           stderr=subprocess.STDOUT, check=True, timeout=300)
    run(["autoreconf", "-fi"], root)
    obj = out / "build"; obj.mkdir()
    flags = ["--disable-shared", "--enable-static", "--disable-multithreading",
        "--enable-lz4", "--disable-lzma", "--without-libzstd", "--without-zlib",
        "--without-libdeflate", "--without-uuid", "--without-xxhash",
        "--without-openssl", "--without-libcurl", "--without-libxml2",
        "--without-json-c", "--without-libnl3", "--disable-s3", "--disable-oci",
        "--disable-fuse", "--disable-fanotify", "--disable-ublk"]
    run([root / "configure", *flags], obj)
    run(["make", "-C", "lib", "-j2", "liberofs.la"], obj)
    library = obj / "lib/.libs/liberofs.a"
    if not library.is_file():
        raise RuntimeError("upstream build did not produce liberofs.a")
    result = {"status": "built", "fingerprint": fingerprint,
              "commit": LOCK["commit"], "library": str(library),
              "library_sha256": hashlib.sha256(library.read_bytes()).hexdigest(),
              "includes": [str(root / "include"), str(root / "lib"), str(obj)],
              "source": str(root), "config": str(obj / "config.h"),
              "log": str(log), "sanitize": sanitize}
    result["tree"] = LOCK["tree"]
    result["artifacts"] = artifact_hashes(library, result["includes"])
    tmpstamp = stamp.with_suffix(".json.tmp")
    tmpstamp.write_text(json.dumps(result, indent=2) + "\n")
    tmpstamp.replace(stamp)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--sanitize", action="store_true")
    a = parser.parse_args()
    try:
        print(json.dumps(build(a.source, a.out, a.sanitize), indent=2))
    except Exception as e:
        print(f"native build failed: {e}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
