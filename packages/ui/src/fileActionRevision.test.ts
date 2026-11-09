import {describe,expect,it,vi} from "vitest";
import {RequestError,type Order} from "@cowork/client";
import {refreshSavedOrder,saveCurrentDraft} from "./fileActionRevision";

const saved={id:"chat",project_id:"project",revision:8,draft:"Retained composer text"} as Order;
describe("chat revision after native actions",()=>{
  it("uses the new revision after a native action without writing over the draft",async()=>{
    const current={...saved,revision:10};const order=vi.fn(async()=>current);
    expect(await refreshSavedOrder({order},saved)).toBe(current);
    expect(order).toHaveBeenCalledExactlyOnceWith("chat");expect(saved.revision).toBe(8);
  });
  it.each([
    {...saved,revision:10,draft:"A different window edited this"},
    {...saved,revision:7},
    {...saved,id:"different-chat"},
    {...saved,project_id:"different-project"},
  ])("refuses changed drafts or mismatched snapshots",async current=>{
    const result=refreshSavedOrder({order:async()=>current},saved);
    await expect(result).rejects.toBeInstanceOf(RequestError);
    await expect(result).rejects.toMatchObject({status:409,details:{code:"revision_conflict"}});
  });
  it("does not fall back to a stale revision when the fresh read fails",async()=>{
    await expect(refreshSavedOrder({order:async()=>{throw new Error("offline");}},saved)).rejects.toThrow("offline");
  });
  it("saves newly edited composer text against the revision after the native action",async()=>{
    const current={...saved,revision:10};const updated={...current,revision:11,draft:"My new text"};
    const saveDraft=vi.fn(async()=>updated);
    expect(await saveCurrentDraft({order:async()=>current,saveDraft},saved,"My new text")).toBe(updated);
    expect(saveDraft).toHaveBeenCalledExactlyOnceWith(saved.id,{base_revision:10,draft:"My new text"});
  });
  it("never writes the local edit over another window's changed draft",async()=>{
    const saveDraft=vi.fn();
    await expect(saveCurrentDraft({order:async()=>({...saved,revision:10,draft:"Remote edit"}),saveDraft},saved,"My new text")).rejects.toMatchObject({status:409});
    expect(saveDraft).not.toHaveBeenCalled();
  });
});
