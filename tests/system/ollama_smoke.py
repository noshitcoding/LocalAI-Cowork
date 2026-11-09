"""Opt-in local-model smoke. Uses only the named local provider and synthetic data; never downloads models."""
from execution_plan_client import reviewed_run
import argparse,json,pathlib,time,urllib.request,uuid
p=argparse.ArgumentParser()
p.add_argument("--data",type=pathlib.Path,required=True)
p.add_argument("--model",required=True)
p.add_argument("--port",type=int,default=43821)
a=p.parse_args()
token=(a.data/"daemon.token").read_text().strip()
def api(method,path,body=None):
    req=urllib.request.Request(f"http://127.0.0.1:{a.port}/api/v2"+path,method=method,
        data=None if body is None else json.dumps(body).encode(),
        headers={"Authorization":"Bearer "+token,"Content-Type":"application/json"})
    with urllib.request.urlopen(req,timeout=20) as response:return json.load(response)
provider=api("POST","/providers",{"name":"Lokaler Modelltest · "+a.model,"kind":"ollama",
    "endpoint":"http://127.0.0.1:11434","model":a.model,"context_tokens":32768})
project=api("POST","/projects",{"name":"Lokaler Modelltest"})
order=api("POST","/orders",{"project_id":project["id"],"title":"Ollama · echte Modellantwort"})
request={"order_id":order["id"],"base_revision":1,"idempotency_key":str(uuid.uuid4()),
    "prompt":"Antworte ausschließlich mit V2_LOCAL_OK. Verwende keine Werkzeuge. /no_think",
    "provider_id":provider["id"],"provider_revision":provider["revision"],"access_mode":"read_only"}
assert api("POST","/preflight",request)["ready"]
run=reviewed_run(api, request)
deadline=time.monotonic()+120
while time.monotonic()<deadline:
    detail=api("GET","/runs/"+run["id"])
    if detail["run"]["state"] in ("completed","failed","canceled","expired","interrupted"):break
    time.sleep(.5)
else:
    api("POST","/runs/"+run["id"]+"/cancel",{})
    raise SystemExit("Real model exceeded 120s; cancellation requested")
assert detail["run"]["state"]=="completed",detail["run"].get("error_code")
messages=api("GET","/orders/"+order["id"]+"/messages")
assert "V2_LOCAL_OK" in messages[-1]["content"],"Unexpected real-model reply"
assert not detail["artifacts"],"Read-only test produced an artifact"
print(json.dumps({"result":"passed","model":a.model,"state":detail["run"]["state"],
    "usage":detail["run"]["usage"],"order_id":order["id"],"run_id":run["id"],
    "scope":"one synthetic local-model response; not a throughput or hardware benchmark"}))

