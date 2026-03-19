# IC20

IC20 is a compiled language for [Stationeers](https://store.steampowered.com/app/544550/Stationeers/) programmable chips. It offers Rust-inspired syntax with named variables, functions, structured control flow, and static type checking, and compiles everything down to tight IC10 assembly that fits within the chip's 128-line limit.

## Modules

### [ic20c](ic20c/) — Compiler

The IC20-to-IC10 compiler. Takes `.ic20` source files and produces IC10 assembly output. The compilation pipeline runs through parsing, name resolution, type checking, SSA-based optimization, register allocation, and code generation.

See the [compiler README](ic20c/README.md) for usage, examples, and compiler flags.

### [ic20-vscode](ic20-vscode/) — VS Code Extension

A VS Code extension providing syntax highlighting, diagnostics, hover info, go-to-definition, formatting, snippets, and an integrated build command for IC20 and IC10 files. It bundles the compiler and language servers for a batteries-included experience.

See the [extension README](ic20-vscode/README.md) for installation and configuration.

### [ic20-lsp](ic20-lsp/) — IC20 Language Server

Language server for `.ic20` files. Provides diagnostics, hover, go-to-definition, rename, and document symbols. Bundled with the VS Code extension.

### [ic10-lsp](ic10-lsp/) — IC10 Language Server

Language server for `.ic10` files. Provides diagnostics, hover, and instruction validation for raw IC10 assembly. Bundled with the VS Code extension.

## Getting Started

### Install from the VS Code Marketplace

Search for **IC20 Language** by Jason Watkins in the Extensions view, or install from the [Marketplace listing](https://marketplace.visualstudio.com/items?itemName=JasonWatkins.ic20-language). The extension bundles the compiler and language servers — no extra setup needed.

### Build and install locally

If you prefer to build from source, run the included build script from the repository root:

```sh
python build-install-extension.py
```

This builds the compiler, both language servers, packages the extension, and installs it into VS Code.

### Compiling an IC20 file

Open a `.ic20` file in VS Code and press `Ctrl+Shift+B`. The compiler writes an `.ic10` file next to the source. Paste the output into a programmable chip in Stationeers.
