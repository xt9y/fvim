"""Minimal LSP fixture for native snippet-completion integration tests."""
import json
import sys


def send(value):
    body = json.dumps(dict(jsonrpc="2.0", **value)).encode()
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode() + body)
    sys.stdout.buffer.flush()


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
        completion = params["capabilities"]["textDocument"]["completion"]["completionItem"]
        assert completion.get("snippetSupport") is True, completion
        send(dict(id=request_id, result=dict(capabilities=dict(
            textDocumentSync=2,
            completionProvider=dict(triggerCharacters=["."]),
        ))))
    elif method == "textDocument/completion":
        pos = params["position"]
        send(dict(id=request_id, result=[dict(
            label="for-loop",
            detail="for (init; condition; inc) { statements }",
            insertTextFormat=2,
            textEdit=dict(
                newText="for (${1:init-statement}; ${2:condition}; ${3:inc-expression}) { ${4:statements} }$0",
                range=dict(start=dict(line=pos["line"], character=0), end=pos),
            ),
        )]))
    elif request_id is not None:
        send(dict(id=request_id, result=None))
