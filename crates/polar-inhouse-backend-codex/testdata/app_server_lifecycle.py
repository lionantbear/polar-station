"""Deterministic JSON-RPC failure fixture; no credentials or model calls."""
import json
import sys

scenario, record_path = sys.argv[1:]
for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    with open(record_path, "a", encoding="utf-8") as record:
        record.write(json.dumps({"method": method}) + "\n")
    if "id" not in request:
        continue
    response = {"id": request["id"], "result": {}}
    if method == "thread/read":
        response["result"] = {"thread": {"id": request["params"]["threadId"], "status": {"type": "active" if scenario == "busy" else "notLoaded"}}}
    elif method == "thread/resume":
        response = {"id": request["id"], "error": {"code": -32603, "message": "fixture resume failure"}}
    elif method == "thread/start":
        response["result"] = {"thread": {"id": "fixture-thread"}}
    elif method == "turn/start":
        if scenario == "dispatch_disconnect":
            sys.exit(0)
        response = {"id": request["id"], "error": {"code": -32603, "message": "fixture dispatch failure"}}
    print(json.dumps(response), flush=True)
