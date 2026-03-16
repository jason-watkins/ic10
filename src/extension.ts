import * as vscode from "vscode";

export function activate(context: vscode.ExtensionContext) {
    context.subscriptions.push(
        vscode.languages.registerDocumentFormattingEditProvider("ic20", {
            provideDocumentFormattingEdits(document, options) {
                return formatDocument(document, options);
            },
        })
    );
}

export function deactivate() { }

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

        if (trimmed === "}" || trimmed.startsWith("} else") || trimmed.startsWith("} else {")) {
            depth = Math.max(0, depth - 1);
        }

        const desired = indent.repeat(depth) + trimmed;
        if (line.text !== desired) {
            edits.push(vscode.TextEdit.replace(line.range, desired));
        }

        const opens = countUnmatchedBraces(trimmed);
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
