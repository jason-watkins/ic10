#!/usr/bin/env python3
"""Builds the IC20 VS Code extension, including the compiler, LSPs, and VSIX package."""

import os
import platform
import shlex
import shutil
import subprocess
import sys
from pathlib import Path
from typing import List, Union


class ScriptError(Exception):
    def __init__(self, msg: str) -> None:
        self.msg = msg


class Logger:
    def __init__(self, quiet: bool = False, debug: bool = False):
        self._quiet = quiet
        self._debug = debug
        if sys.stdout.isatty():
            self._blue = "\033[38;5;4m"
            self._reset = "\033[39m"
            self._yellow = "\033[38;5;3m"
            self._red = "\033[38;5;1m"
            self._magenta = "\033[38;5;5m"
        else:
            self._blue = ""
            self._reset = ""
            self._yellow = ""
            self._red = ""
            self._magenta = ""

    def _log(self, color: str, pre: str, msg: str, dst=sys.stdout) -> None:
        if self._quiet and dst == sys.stdout:
            return
        dst.write(f"[{color}{pre}{self._reset}] {msg}\n")

    def dbg(self, msg: str) -> None:
        if self._debug:
            self._log(self._magenta, "DBUG", msg)

    def ok(self, msg: str) -> None:
        self._log(self._blue, " OK ", msg)

    def info(self, msg: str) -> None:
        self._log(self._reset, "INFO", msg)

    def warn(self, msg: str) -> None:
        self._log(self._yellow, "WARN", msg)

    def err(self, msg: str) -> None:
        self._log(self._red, "FAIL", msg, sys.stderr)


log = Logger()


def run(args: Union[str, List[str]], *, cwd: Union[Path, None] = None) -> bytes:
    shell = isinstance(args, str)
    display = args if shell else shlex.join(args)
    log.dbg(f"Running: {display}")

    try:
        process = subprocess.run(args, shell=shell, capture_output=True, check=True, cwd=cwd)
        return process.stdout
    except subprocess.CalledProcessError as ex:
        log.err(f"Failed to run {display}")
        log.err(f"    Return code: {ex.returncode}")
        if ex.stdout:
            log.err(f"    STDOUT:")
            sys.stdout.buffer.write(ex.stdout)
            sys.stdout.write("\n")
        if ex.stderr:
            log.err(f"    STDERR:")
            sys.stderr.buffer.write(ex.stderr)
            sys.stderr.write("\n")
        raise ScriptError(f"Command failed with exit code {ex.returncode}: {display}") from ex


def get_platform() -> str:
    system = platform.system()
    if system == "Windows":
        return "win32"
    if system == "Darwin":
        return "darwin"
    return "linux"


def get_arch() -> str:
    machine = platform.machine().lower()
    if machine in ("arm64", "aarch64"):
        return "arm64"
    return "x64"


def get_exe_suffix() -> str:
    if platform.system() == "Windows":
        return ".exe"
    return ""


def cargo_build(crate_dir: Path) -> None:
    run(["cargo", "build", "--release"], cwd=crate_dir)


def main() -> None:
    root = Path(__file__).resolve().parent
    lsp_dir = root / "ic20-lsp"
    ic10_lsp_dir = root / "ic10-lsp"
    compiler_dir = root / "ic20c"
    extension_dir = root / "ic20-vscode"
    bin_dir = extension_dir / "bin"

    plat = get_platform()
    arch = get_arch()
    ext = get_exe_suffix()

    log.info("Building ic20-lsp (release)")
    cargo_build(lsp_dir)
    log.ok("Built ic20-lsp")

    log.info("Building ic10-lsp (release)")
    cargo_build(ic10_lsp_dir)
    log.ok("Built ic10-lsp")

    log.info("Building ic20c (release)")
    cargo_build(compiler_dir)
    log.ok("Built ic20c")

    log.info("Copying binaries to extension bin/")
    bin_dir.mkdir(parents=True, exist_ok=True)

    binaries = [
        ("ic20-lsp", lsp_dir),
        ("ic10-lsp", ic10_lsp_dir),
        ("ic20c", compiler_dir),
    ]

    for name, crate_dir in binaries:
        source = crate_dir / "target" / "release" / f"{name}{ext}"
        destination = bin_dir / f"{name}-{plat}-{arch}{ext}"
        shutil.copy2(str(source), str(destination))
        log.ok(f"Copied {destination.name}")

    npm = "npm.cmd" if platform.system() == "Windows" else "npm"
    npx = "npx.cmd" if platform.system() == "Windows" else "npx"

    log.info("Installing npm dependencies")
    run([npm, "ci"], cwd=extension_dir)
    log.ok("Installed npm dependencies")

    log.info("Bundling extension")
    run([npm, "run", "bundle"], cwd=extension_dir)
    log.ok("Bundled extension")

    log.info("Packaging VSIX")
    run([npx, "vsce", "package"], cwd=extension_dir)
    log.ok("Packaged VSIX")

    vsix_files = sorted(extension_dir.glob("*.vsix"), key=lambda p: p.stat().st_mtime, reverse=True)
    if not vsix_files:
        raise ScriptError("No .vsix file found after packaging")

    vsix = vsix_files[0]
    log.info(f"Installing extension from {vsix.name}")

    code = "code.cmd" if platform.system() == "Windows" else "code"
    run([code, "--profile", "Rust", "--install-extension", str(vsix)], cwd=extension_dir)
    log.ok("Installed extension")

    log.ok("Done. Run 'Developer: Reload Window' in VS Code (Ctrl+Shift+P) to activate the new version.")


if __name__ == "__main__":
    try:
        main()
    except ScriptError as ex:
        log.err(ex.msg)
        sys.exit(1)
    except KeyboardInterrupt:
        log.warn("Interrupted")
        sys.exit(130)
