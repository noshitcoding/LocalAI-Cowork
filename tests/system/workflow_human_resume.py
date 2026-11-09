"""Actual daemon hard-kill and explicit journal human continuation checks.

Only disposable fixture data and this script's child daemons are used.
Synthetic loopback HTTP is counted; no system clock changes or stack overrides.
"""
import argparse
import contextlib
import http.server
import io
import json
import os
import pathlib
import sqlite3
import tempfile
import threading
import time
from workflow_inputs import Fixture, Provider, ANSWERS


class ResumeProvider(Provider):
    catalogue_calls=0
    multi=False
    hold_step=None
    release=threading.Event()

    def do_GET(self):
        type(self).catalogue_calls+=1
        super().do_GET()

    def do_POST(self):
        body_bytes=self.rfile.read(int(self.headers["Content-Length"]))
        body=json.loads(body_bytes)
        step=None
        for message in body["messages"]:
            try: value=json.loads(message.get("content") or "")
            except (json.JSONDecodeError,TypeError): continue
            if isinstance(value,dict) and "workflow_step" in value: step=value["workflow_step"]
        if step==type(self).hold_step:
            with self.lock: self.requests.append(body)
            type(self).release.wait(timeout=120)
            return
        if step=="ask_a" and type(self).multi:
            with self.lock: self.requests.append(body)
            messages=[m for m in body["messages"] if m["role"]=="tool"]
            delta={"content":"Synthetic completed ask_a"}
            if not messages:
                delta["tool_calls"]=[{"index":i,"id":raw,"type":"function","function":{"name":"request_input","arguments":json.dumps({"question":"Question A "+str(i)+"?"})}} for i,raw in enumerate(("same-raw-call","second-raw-call"))]
            else:
                assert [m["tool_call_id"] for m in messages]==["same-raw-call","second-raw-call"],messages
                assert all(m["content"]==ANSWERS["ask_a"] for m in messages),messages
            payload={"choices":[{"index":0,"delta":delta,"finish_reason":"tool_calls" if not messages else "stop"}],"usage":{"prompt_tokens":64,"completion_tokens":16}}
            self.send_response(200);self.send_header("Content-Type","text/event-stream");self.end_headers()
            self.wfile.write(("data: "+json.dumps(payload)+"\n\ndata: [DONE]\n\n").encode())
            return
        self.rfile=io.BytesIO(body_bytes)
        super().do_POST()


def stored(f, run):
    with contextlib.closing(sqlite3.connect(f.data / "cowork-v2.sqlite3")) as db:
        return json.loads(db.execute("SELECT payload FROM runs WHERE id=?", (run,)).fetchone()[0]), json.loads(db.execute("SELECT payload FROM workflow_runs WHERE run_id=?", (run,)).fetchone()[0])


def prepare_resume(f, run, key):
    view = f.call("GET", f"/runs/{run}/workflow")
    p = view["input_resume_preview"]
    assert p["available"], view
    request = {"run_id": run, "base_run_revision": p["base_run_revision"], "base_workflow_revision": p["base_workflow_revision"], "expected_checkpoint_sha256": p["expected_checkpoint_sha256"], "expected_cursor_sha256": p["expected_cursor_sha256"], "idempotency_key": key}
    path = f"/projects/{f.project['id']}/workflow-operations"
    operation = f.call("POST", path, {"intent_id": key, "request": {"kind": "resume_input", "request": request}, "draft_snapshot": None})
    return path + "/" + operation["id"], operation


def recover_offline(f, run):
    f.kill(); f.start()
    f.wait(f"/runs/{run}",lambda v:v["run"]["state"]=="interrupted")
    f.kill(); f.start(no_worker=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=pathlib.Path, default=pathlib.Path("target/debug/cowork-daemon.exe" if os.name == "nt" else "target/debug/cowork-daemon"))
    binary = parser.parse_args().binary.resolve()
    provider = http.server.ThreadingHTTPServer(("127.0.0.1", 0), ResumeProvider)
    threading.Thread(target=provider.serve_forever, daemon=True).start()
    checks = []
    with tempfile.TemporaryDirectory(prefix="cowork-v2-human-resume-") as temp:
        f = Fixture(binary, pathlib.Path(temp), provider)
        try:
            f.start(); f.initialize()
            ResumeProvider.multi=True
            order, run = f.run("Actual hard kill with two human questions", "read_only")
            questions = f.questions(run)
            f.wait(f"/runs/{run}/workflow", lambda v: any(n["id"] == "independent" and n["state"] == "completed" for n in v["nodes"]))
            for node,q in questions.items():
                f.save(run,q,ANSWERS[node],"draft-"+node)
            intents = {node:f.prepare(run,q,1,"answer-"+node) for node,q in questions.items()}
            original_parent,original_workflow=stored(f,run)
            before=Provider.count()
            catalogue_before=ResumeProvider.catalogue_calls
            recover_offline(f,run)
            path,operation=prepare_resume(f,run,"resume-one")
            f.kill(); f.start(no_worker=True)
            assert f.call("GET",path)==operation
            assert f.call("GET",f"/runs/{run}")["run"]["state"]=="interrupted"
            assert Provider.count()==before and ResumeProvider.catalogue_calls==catalogue_before
            f.lost_ack("POST",path+"/commit")
            accepted=f.call("GET",path)
            assert accepted["state"]=="accepted" and accepted["receipt"]["generation"]==1,accepted
            assert f.call("POST",path+"/commit")==accepted
            queued,workflow=stored(f,run)
            assert queued["record"]["started_at"]==original_parent["record"]["started_at"] and queued["lease"] is None
            assert workflow["progress"]["human_continuations"][-1]["execution_claims"]==[]
            assert Provider.count()==before
            assert ResumeProvider.catalogue_calls==catalogue_before
            for node,intent in intents.items(): assert f.call("GET",f.intent_path(run,intent))==intent
            checks.append("TX A lost acknowledgement returns one exact accepted generation and preserves original start, pending answers and provider request count")
            f.kill(); f.start()
            view=f.wait(f"/runs/{run}/workflow",lambda v:all(q["actionable"] for q in v["inputs"]))
            activated,workflow=stored(f,run)
            c=workflow["progress"]["human_continuations"][-1]
            assert c["generation"]==1 and len(c["execution_claims"])==2 and activated["lease"]["token"]!=original_parent["lease"]["token"]
            assert [q["request_id"] for q in view["inputs"]]==[q["request_id"] for q in sorted(questions.values(),key=lambda q:q["request_id"])]
            assert all(q["expires_at"]==questions[q["node_id"]]["expires_at"] and q["fingerprint"]==questions[q["node_id"]]["fingerprint"] for q in view["inputs"])
            time.sleep(.4)
            assert Provider.count()==before
            assert ResumeProvider.catalogue_calls==catalogue_before
            for node,intent in intents.items(): assert f.call("GET",f.intent_path(run,intent))==intent
            checks.append("TX B activates exactly generation one with fresh lease/claims, same question identities/deadlines and no old request replay or automatic answer")
            # Generation one opens another human question in the SAME old
            # confirmed model response, under its new execution claim.
            first_answer=f.call("POST",f.intent_path(run,intents["ask_a"])+"/commit")
            assert first_answer["state"]=="accepted"
            second_question=f.wait(f"/runs/{run}/workflow",lambda v:len(v["inputs"])==3)
            q2=next(q for q in second_question["inputs"] if q["node_id"]=="ask_a" and q["actionable"])
            f.save(run,q2,ANSWERS["ask_a"],"second-question-draft")
            next_intent=f.prepare(run,q2,1,"second-question-answer")
            intents["ask_a"]=next_intent
            assert Provider.count()==before
            # A second hard exit must create N+1, never reactivate N.
            recover_offline(f,run)
            second_path,_=prepare_resume(f,run,"resume-two")
            second=f.call("POST",second_path+"/commit")
            assert second["receipt"]["generation"]==2 and f.call("POST",path+"/commit")==accepted
            f.kill(); f.start()
            f.wait(f"/runs/{run}/workflow",lambda v:sum(q["actionable"] for q in v["inputs"])==2)
            _,bridge=stored(f,run)
            second_cursor=next(c for c in bridge["progress"]["human_continuations"][-1]["cursors"] if c["node_id"]=="ask_a")
            assert second_cursor["model_claim"]!=second_cursor["input_claim"]
            for node,intent in intents.items():
                result=f.call("POST",f.intent_path(run,intent)+"/commit")
                assert result["state"]=="accepted",result
            f.wait(f"/runs/{run}",lambda v:v["run"]["state"]=="completed")
            assert Provider.count()==before+2 and not Provider.errors,(Provider.count(),Provider.errors)
            events=f.call("GET",f"/runs/{run}/events?after=0")
            assert len([e for e in events if e["kind"]=="workflow_input_resume_activated"])==2
            assert len([e for e in events if e["kind"]=="workflow_input_answered"])==3
            final_parent,final_workflow=stored(f,run)
            assert final_parent["record"]["started_at"]==original_parent["record"]["started_at"]
            assert all(c["consumed"] for c in final_workflow["progress"]["human_continuations"][-1:])
            assert f.call("POST",path+"/commit")==accepted
            checks.append("Generation one opens a second question in the same old response; generation two distinguishes original model claim, prior human owner and new execution authority; new requests and exact answers settle once")
            ResumeProvider.multi=False
            ResumeProvider.hold_step="ask_b"
            _,unknown=f.run("One human question and one unknown parallel HTTP request","read_only")
            partial=f.wait(f"/runs/{unknown}/workflow",lambda v:len(v["inputs"])==1 and any(n["id"]=="independent" and n["state"]=="completed" for n in v["nodes"]))
            unknown_q=partial["inputs"][0]
            f.save(unknown,unknown_q,ANSWERS["ask_a"],"unknown-draft")
            unknown_intent=f.prepare(unknown,unknown_q,1,"unknown-answer")
            count=Provider.count(); catalogue=ResumeProvider.catalogue_calls
            recover_offline(f,unknown)
            view=f.call("GET",f"/runs/{unknown}/workflow")
            p=view["input_resume_preview"]
            assert not p["available"] and p["blocked_reason"]=="uncertain_effect",view
            operation=f.call("POST",f"/projects/{f.project['id']}/workflow-operations",{"intent_id":"unsafe-resume","request":{"kind":"resume_input","request":{"run_id":unknown,"base_run_revision":p["base_run_revision"],"base_workflow_revision":p["base_workflow_revision"],"expected_checkpoint_sha256":p["expected_checkpoint_sha256"],"expected_cursor_sha256":"0"*64,"idempotency_key":"unsafe-resume"}},"draft_snapshot":None})
            f.error("POST",f"/projects/{f.project['id']}/workflow-operations/{operation['id']}/commit",None,409,"uncertain_effect")
            assert f.call("GET",f.intent_path(unknown,unknown_intent))==unknown_intent
            parent,graph=stored(f,unknown)
            assert parent["record"]["state"]=="interrupted" and parent["lease"] is None
            assert any(a["outcome"] is None and a["operation"]["operation"]=="model" for a in graph["progress"]["attempts"].values())
            assert Provider.count()==count and ResumeProvider.catalogue_calls==catalogue
            ResumeProvider.release.set(); ResumeProvider.hold_step=None
            checks.append("Actual unknown parallel HTTP admission blocks whole-run resume before provider catalogue/request I/O; original unconfirmed reservation and pending answer remain unchanged")
            # Revocation after a committed acceptance is a typed nonactivation.
            f.kill(); f.start()
            for mode,reason in (("missing","workflow_clock_unverified"),("future","workflow_clock_rollback")):
                _,clock_run=f.run("Accepted resume then "+mode+" clock", "read_only")
                clock_questions=f.questions(clock_run)
                f.wait(f"/runs/{clock_run}/workflow",lambda v:any(n["id"]=="independent" and n["state"]=="completed" for n in v["nodes"]))
                q=clock_questions["ask_a"]
                f.save(clock_run,q,ANSWERS["ask_a"],"clock-draft-"+mode)
                pending_answer=f.prepare(clock_run,q,1,"clock-answer-"+mode)
                recover_offline(f,clock_run)
                clock_path,_=prepare_resume(f,clock_run,"clock-resume-"+mode)
                frozen=f.call("POST",clock_path+"/commit")
                untouched=stored(f,clock_run)
                count=Provider.count();catalogue=ResumeProvider.catalogue_calls
                f.kill()
                database=(f.data/"cowork-v2.sqlite3").resolve()
                assert database.parent==f.data.resolve() and f.process is None
                with contextlib.closing(sqlite3.connect(database)) as db:
                    row=db.execute("SELECT highwater_ms,payload FROM workflow_execution_clocks WHERE run_id=?",(clock_run,)).fetchone()
                    projection=json.loads(row[1])
                    assert projection["run_id"]==clock_run and projection["highwater_ms"]==row[0]
                    if mode=="missing": db.execute("DELETE FROM workflow_execution_clocks WHERE run_id=?",(clock_run,))
                    else:
                        projection["highwater_ms"]=row[0]+3600000
                        db.execute("UPDATE workflow_execution_clocks SET highwater_ms=?,payload=? WHERE run_id=?",(projection["highwater_ms"],json.dumps(projection),clock_run))
                    db.commit()
                f.start()
                f.wait(f"/runs/{clock_run}",lambda v:v["run"]["state"]=="interrupted")
                closed,graph=stored(f,clock_run)
                assert closed["lease"] is None and graph["progress"]["human_continuations"][-1]["blocked_reason"]==reason
                assert closed["record"]["model_attempts"]==untouched[0]["record"]["model_attempts"] and graph["agents"]==untouched[1]["agents"]
                assert closed["record"]["started_at"]==untouched[0]["record"]["started_at"]
                assert f.call("POST",clock_path+"/commit")==frozen
                assert f.call("GET",f.intent_path(clock_run,pending_answer))==pending_answer
                f.kill();f.start()
                time.sleep(.3)
                repeated,graph=stored(f,clock_run)
                assert repeated["record"]["last_sequence"]==closed["record"]["last_sequence"]
                assert Provider.count()==count and ResumeProvider.catalogue_calls==catalogue
                with contextlib.closing(sqlite3.connect(database)) as db:
                    current=db.execute("SELECT highwater_ms FROM workflow_execution_clocks WHERE run_id=?",(clock_run,)).fetchone()
                    assert (current is None) if mode=="missing" else (current[0]==projection["highwater_ms"])
                events=f.call("GET",f"/runs/{clock_run}/events?after=0")
                assert len([e for e in events if e["kind"]=="workflow_input_resume_blocked"])==1
                assert not any(e["kind"]=="workflow_input_resume_activated" for e in events)
                checks.append("Accepted resume with "+mode+" private clock remains nonexecuting across actual restarts: one exact typed block, no provider/catalogue/autoanswer, immutable receipt and original start; missing origin stays missing and future HWM stays sticky")
            ResumeProvider.multi=False
            _,revoked=f.run("Accepted then revoked", "read_only")
            f.questions(revoked)
            f.wait(f"/runs/{revoked}/workflow",lambda v:any(n["id"]=="independent" and n["state"]=="completed" for n in v["nodes"]))
            recover_offline(f,revoked)
            revoke_path,_=prepare_resume(f,revoked,"resume-before-revoke")
            revoke_receipt=f.call("POST",revoke_path+"/commit")
            current=f.call("GET",f"/projects/{f.project['id']}/workflows/{f.library['id']}")
            f.call("POST",f"/projects/{f.project['id']}/workflows/{f.library['id']}/revoke",{"base_revision":current["revision"],"idempotency_key":"revoke-now"})
            untouched=stored(f,revoked)
            count=Provider.count()
            f.kill(); f.start()
            f.wait(f"/runs/{revoked}",lambda v:v["run"]["state"]=="interrupted")
            closed,graph=stored(f,revoked)
            assert closed["lease"] is None and graph["progress"]["human_continuations"][-1]["blocked_reason"]=="workflow_revoked"
            assert closed["record"]["model_attempts"]==untouched[0]["record"]["model_attempts"] and graph["agents"]==untouched[1]["agents"]
            assert Provider.count()==count and f.call("POST",revoke_path+"/commit")==revoke_receipt
            events=f.call("GET",f"/runs/{revoked}/events?after=0")
            assert len([e for e in events if e["kind"]=="workflow_input_resume_blocked"])==1
            checks.append("Prepare/Accepted/Revoke/actual queue worker leaves one typed nonactivation, no lease/provider effect, original accepted receipt and charges unchanged")
        finally:
            ResumeProvider.release.set()
            f.kill()
    provider.shutdown(); provider.server_close()
    print(json.dumps({"result":"passed","checks":checks,"processes_stopped":True,"normal_thread_stack":True},ensure_ascii=False,indent=2))


if __name__=="__main__": main()
