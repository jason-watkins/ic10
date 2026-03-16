import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";
import {
    LanguageClient,
    LanguageClientOptions,
    ServerOptions,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;

export async function activate(context: vscode.ExtensionContext) {
    context.subscriptions.push(
        vscode.languages.registerDocumentFormattingEditProvider("ic20", {
            provideDocumentFormattingEdits(document, options) {
                return formatDocument(document, options);
            },
        })
    );

    const config = vscode.workspace.getConfiguration("ic20.lsp");
    const configuredPath = config.get<string>("path", "");

    let command: string;
    if (configuredPath) {
        command = configuredPath;
    } else {
        const ext = process.platform === "win32" ? ".exe" : "";
        const bundledName = `ic20-lsp-${process.platform}-${process.arch}${ext}`;
        const bundledPath = path.join(context.extensionPath, "bin", bundledName);
        command = fs.existsSync(bundledPath) ? bundledPath : "ic20-lsp";
    }

    const serverOptions: ServerOptions = {
        command,
        args: [],
    };

    const clientOptions: LanguageClientOptions = {
        documentSelector: [{ scheme: "file", language: "ic20" }],
    };

    client = new LanguageClient(
        "ic20-lsp",
        "IC20 Language Server",
        serverOptions,
        clientOptions
    );

    try {
        await client.start();
    } catch {
        client = undefined;
        vscode.window.showWarningMessage(
            `Could not start IC20 language server ('${command}'). ` +
            "Diagnostics, hover, and go-to-definition will be unavailable. " +
            "Set ic20.lsp.path in settings or add ic20-lsp to your PATH."
        );
    }
}

export async function deactivate() {
    if (client) {
        await client.stop();
    }
}

function formatDocument(
    document: vscode.TextDocument,
    options: vscode.FormattingOptions
): vscode.TextEdit[] {
    const edits: vscode.TextEdit[] = [];
    const indent = options.insertSpaces ? " ".repeat(options.tabSize) : "\t";
    let depth = 0;

    for (let i = 0; i < document.lineCount; i++) {
        const line = document.lineAt(i);
        const trimmed = line.text.trim();

        if (trimmed.length === 0) {
            if (line.text.length > 0) {
                edits.push(vscode.TextEdit.replace(line.range, ""));
            }
            continue;
        }

        const leadingClose = trimmed.startsWith("}");
        if (leadingClose) {
            depth = Math.max(0, depth - 1);
        }

        const desired = indent.repeat(depth) + trimmed;
        if (line.text !== desired) {
            edits.push(vscode.TextEdit.replace(line.range, desired));
        }

        // When the line started with `}` the pre-decrement already handled that
        // brace, so skip it when computing the depth adjustment for the next line.
        const braceSource = leadingClose ? trimmed.slice(1) : trimmed;
        const opens = countUnmatchedBraces(braceSource);
        depth = Math.max(0, depth + opens);
    }

    const lastLine = document.lineAt(document.lineCount - 1);
    if (lastLine.text.length > 0 && !lastLine.text.endsWith("\n")) {
        edits.push(vscode.TextEdit.insert(lastLine.range.end, "\n"));
    }

    return edits;
}

function countUnmatchedBraces(line: string): number {
    let count = 0;
    let inString = false;
    let inLineComment = false;

    for (let i = 0; i < line.length; i++) {
        const ch = line[i];
        const next = i + 1 < line.length ? line[i + 1] : "";

        if (inLineComment) break;

        if (ch === "/" && next === "/") {
            inLineComment = true;
            break;
        }

        if (ch === '"' && !inLineComment) {
            inString = !inString;
            continue;
        }

        if (inString) continue;

        if (ch === "{") count++;
        else if (ch === "}") count--;
    }

    return count;
}
