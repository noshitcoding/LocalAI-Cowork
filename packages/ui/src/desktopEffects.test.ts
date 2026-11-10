import {afterEach,describe,expect,it,vi} from "vitest";
import {announceCompletion} from "./desktopEffects";
afterEach(()=>vi.unstubAllGlobals());
describe("automatic completion notification permissions",()=>{
 it("quietly skips permissions that have not been granted",async()=>{const notification=vi.fn();Object.assign(notification,{permission:"default"});vi.stubGlobal("Notification",notification);await expect(announceCompletion({notifications:true,sound:false,title:"Done",body:"Completed",tag:"r"})).resolves.toBeUndefined();expect(notification).not.toHaveBeenCalled();});
 it("reports a real delivery failure after permission was granted",async()=>{const notification=vi.fn(()=>{throw new Error("OS delivery failed");});Object.assign(notification,{permission:"granted"});vi.stubGlobal("Notification",notification);await expect(announceCompletion({notifications:true,sound:false,title:"Done",body:"Completed",tag:"r"})).rejects.toThrow("OS delivery failed");});
});
