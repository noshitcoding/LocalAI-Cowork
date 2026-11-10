import {describe,expect,it} from "vitest";
import type {FolderGrant,WorkspaceTask} from "@cowork/client";
import {defaultFileWatch,eligibleWatchFolders,fileWatchMessage,watchExtensions} from "./fileWatchUi";
const folder:FolderGrant={id:"folder",project_id:"project",name:"Inbox",revision:"r1",resource_id:"resource",writable:true};
const task={value:{folders:[folder]}} as WorkspaceTask;
describe("file observation configuration",()=>{
  it("offers only current task-selected external grants and narrows them to read-only",()=>{
    expect(eligibleWatchFolders(task,[folder,{...folder,id:"other"},{...folder,revision:"r2"},{...folder,resource_id:"replacement"},{...folder,order_id:"managed-chat"}])).toEqual([{...folder,writable:false}]);
    expect(eligibleWatchFolders(undefined,[folder])).toEqual([]);
    expect(folder.writable).toBe(true);
  });
  it("requires explicit existing-file processing and keeps drafts independent",()=>{
    const first=defaultFileWatch(folder),second=defaultFileWatch(folder);
    expect(first.include_existing).toBe(false);expect(first.changes).toEqual(["created"]);expect(first.folder.writable).toBe(false);
    first.changes.push("removed");first.folder.name="Draft only";
    expect(second.changes).toEqual(["created"]);expect(second.folder.name).toBe("Inbox");
  });
  it("accepts human file-type lists and explains incomplete observations",()=>{
    expect(watchExtensions("PDF, .PNG, ,docx")).toEqual([".pdf",".png",".docx"]);
    expect(fileWatchMessage("automation_watch_io_failed","de")).toContain("keine Löschereignisse");
    expect(fileWatchMessage("automation_watch_folder_unavailable","en")).toContain("bind the folder again");
  });
});
