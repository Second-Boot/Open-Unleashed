#!/usr/bin/env python3
"""Write THIRD-PARTY-LICENSES.txt for the game binary: every crate the `open-unleashed` package links
(normal dependencies, transitively, for one target), with its SPDX expression and the licence files
its source ships. Build-time-only crates (build scripts, proc macros) are not in the binary and are
left out.

    third_party_licenses.py <out.txt> [--target <triple>] [--dxc <dir with the unpacked DXC release>]

`--dxc` (the Windows archive): the DirectX Shader Compiler's dxcompiler.dll + dxil.dll ship beside the exe, so
their licence files (LICENSE-*.txt of the release) are appended as one more entry.
"""
import json, pathlib, subprocess, sys

LICENCE_PREFIXES = ("license", "licence", "copying", "notice", "unlicense", "copyright")


def licence_files(manifest_dir: pathlib.Path):
    found = [p for p in sorted(manifest_dir.iterdir()) if p.is_file() and p.name.lower().startswith(LICENCE_PREFIXES)]
    if not found and manifest_dir.parent.name == "third_party":
        # the vendored crates keep one licence beside them (third_party/LICENSE.MIT)
        found = [p for p in sorted(manifest_dir.parent.iterdir()) if p.is_file() and p.name.lower().startswith(LICENCE_PREFIXES)]
    return found


def main():
    args = sys.argv[1:]
    if not args:
        sys.exit(__doc__)
    out = pathlib.Path(args[0])
    cmd = ["cargo", "metadata", "--format-version", "1", "--locked"]
    if "--target" in args:
        cmd += ["--filter-platform", args[args.index("--target") + 1]]
    meta = json.loads(subprocess.run(cmd, check=True, capture_output=True, text=True).stdout)
    pkgs = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    members = set(meta["workspace_members"])
    root = next(i for i in members if pkgs[i]["name"] == "open-unleashed")
    seen, stack = set(), [root]
    while stack:
        i = stack.pop()
        if i in seen:
            continue
        seen.add(i)
        for d in nodes[i]["deps"]:
            if any(k["kind"] is None for k in d["dep_kinds"]):
                stack.append(d["pkg"])
    third = sorted((pkgs[i] for i in seen if i not in members and "proc-macro" not in pkgs[i]["targets"][0]["kind"]),
                   key=lambda p: (p["name"], p["version"]))
    parts = [
        "Third-party software in this program\n",
        "=====================================\n\n",
        "The program itself is licensed under the GNU General Public License v3.0 or later (LICENSE).\n",
        "It links the following crates, each under its own licence (SPDX expression; for an \"A OR B\"\n",
        "expression the crate may be used under either). Their licence texts follow.\n\n",
    ]
    for p in third:
        parts.append(f"  {p['name']} {p['version']}  —  {p.get('license') or 'see licence file'}\n")
    for p in third:
        d = pathlib.Path(p["manifest_path"]).parent
        parts.append("\n" + "=" * 78 + f"\n{p['name']} {p['version']}  ({p.get('license') or 'see below'})\n")
        if p.get("repository"):
            parts.append(f"{p['repository']}\n")
        parts.append("=" * 78 + "\n")
        files = licence_files(d)
        if p["name"] == "sdl3-sys":
            # the crate builds SDL3 from the source of its build dependency `sdl3-src` and links it
            # statically (zlib licence); sdl3-src is build-time only, so it is not in the list above
            src = next((q for q in meta["packages"] if q["name"] == "sdl3-src"), None)
            lic = pathlib.Path(src["manifest_path"]).parent / "SDL" / "LICENSE.txt" if src else None
            if lic is None or not lic.is_file():
                sys.exit("sdl3-src's SDL/LICENSE.txt not found: the SDL3 licence must ship with the binary")
            parts.append(f"This crate statically links SDL3 (https://libsdl.org), built from the source of sdl3-src {src['version']}; its licence (zlib) follows.\n")
            files = files + [lic]
        if not files:
            authors = ", ".join(p.get("authors") or []) or "its authors"
            parts.append(f"(the crate's source ships no licence file. Licence: {p.get('license')}; copyright {authors}.\n"
                         f" The standard text of that licence is in the entry of any crate above or below that uses it.)\n")
        for f in files:
            parts.append(f"\n--- {f.name} ---\n\n")
            parts.append(f.read_text(errors="replace").rstrip() + "\n")
    if "--dxc" in args:
        d = pathlib.Path(args[args.index("--dxc") + 1])
        files = sorted(d.glob("LICENSE-*.txt"))
        if not files:
            sys.exit(f"no LICENSE-*.txt of the DirectX Shader Compiler in {d}: its licence must ship with dxcompiler.dll")
        parts.append("\n" + "=" * 78 + "\nDirectX Shader Compiler (dxcompiler.dll, dxil.dll beside the executable)  (Apache-2.0 WITH LLVM-exception, MIT, Microsoft licences)\n")
        parts.append("https://github.com/microsoft/DirectXShaderCompiler\n" + "=" * 78 + "\n")
        parts.append("Shipped as the unmodified official x64 binaries (separate files, loaded at run time); the licence texts of the release follow.\n")
        for f in files:
            parts.append(f"\n--- {f.name} ---\n\n")
            parts.append(f.read_text(errors="replace").rstrip() + "\n")
    out.write_text("".join(parts))
    print(f"{out}: {len(third)} crates")


if __name__ == "__main__":
    main()
