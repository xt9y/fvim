"""Deterministic subprocess fixtures for the installed editor's LSP/build tests."""
import json
import sys

if len(sys.argv) > 1 and sys.argv[1] == "--build":
    if "--interactive" in sys.argv:
        assert sys.stdin.isatty() and sys.stdout.isatty(), "Build needs a real terminal"
        print("INPUT_PROMPT: ", end="", flush=True)
        print("INPUT_RECEIVED: " + input(), flush=True)
    if "--wait" in sys.argv:
        import time
        print("WAITING_FOR_CANCEL", flush=True)
        while True:
            time.sleep(1)
    print("BUILD_ARGS: " + json.dumps(sys.argv[2:]), flush=True)
    print("language.c:1:5: error: fixture compiler error", file=sys.stderr, flush=True)
    sys.exit(1)


def send(value):
    body = json.dumps(dict(jsonrpc="2.0", **value)).encode()
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode() + body)
    sys.stdout.buffer.flush()


documents = {}
while True:
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            sys.exit(0)
        if line in (b"\r\n", b"\n"):
            break
        key, _, value = line.partition(b":")
        if key.lower() == b"content-length":
            length = int(value)
    message = json.loads(sys.stdin.buffer.read(length))
    method = message.get("method", "")
    params = message.get("params", {})
    request_id = message.get("id")
    if method == "initialize":
        send(dict(id=request_id, result=dict(capabilities=dict(textDocumentSync=2,
                  completionProvider=dict(triggerCharacters=["."]), hoverProvider=True,
                  definitionProvider=True, referencesProvider=True))))
    elif method in ("textDocument/didOpen", "textDocument/didChange"):
        doc = params["textDocument"]
        documents[doc["uri"]] = doc
        send(dict(method="textDocument/publishDiagnostics", params=dict(uri=doc["uri"],
                  version=doc["version"], diagnostics=[dict(range=dict(start=dict(line=0, character=4),
                  end=dict(line=0, character=8)), severity=1, source="fixture-lsp", message="fixture LSP diagnostic")])))
    elif method == "textDocument/didSave":
        doc = documents[params["textDocument"]["uri"]]
        send(dict(method="textDocument/publishDiagnostics", params=dict(uri=doc["uri"],
                  version=doc["version"], diagnostics=[dict(range=dict(start=dict(line=0, character=4),
                  end=dict(line=0, character=8)), severity=1, source="fixture-lsp", message="fixture saved diagnostic")])))
    elif method == "textDocument/completion":
        pos = params["position"]
        send(dict(id=request_id, result=[dict(label="fixture_completion", insertText="fixture_completion",
                  textEdit=dict(newText="fixture_completion", range=dict(start=dict(line=pos["line"], character=0), end=pos)))]))
    elif method == "textDocument/hover":
        send(dict(id=request_id, result=dict(contents=dict(kind="plaintext", value="fixture hover documentation"))))
    elif method in ("textDocument/definition", "textDocument/references"):
        send(dict(id=request_id, result=[dict(uri=params["textDocument"]["uri"],
                  range=dict(start=dict(line=0, character=0), end=dict(line=0, character=3)))]))
    elif request_id is not None:
        send(dict(id=request_id, result=None))
