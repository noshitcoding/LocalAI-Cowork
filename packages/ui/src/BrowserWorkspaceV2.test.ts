import {describe,expect,it} from "vitest";
import {browserPointer} from "./BrowserWorkspaceV2";
import {browserSetupRequired,browserSetupMessage} from './browserReadiness';
describe('browser setup admission',()=>{
  it('requires the selected managed engine and Python, independently of legacy Chromium',()=>{
    expect(browserSetupRequired(['python','chromium'],'chrome','managed')).toBe(true);
    expect(browserSetupRequired(['python','chrome'],'chrome','managed')).toBe(false);
    expect(browserSetupRequired(['python','chrome'],'firefox','managed')).toBe(true);
    expect(browserSetupRequired(['chrome'],'chrome','managed')).toBe(true);
    expect(browserSetupRequired([],'firefox','extension')).toBe(false);
  });
  it('maps setup and busy-profile errors to actionable messages in both languages',()=>{
    expect(browserSetupMessage('browser_install_runtime_first','de')).toContain('Playwright');
    expect(browserSetupMessage('browser_profile_busy','en')).toContain('Close');
    expect(browserSetupMessage('unknown failure','en')).toBeUndefined();
  });
});
describe("browser viewport input",()=>{
  it("maps scaled content coordinates to the exact browser viewport",()=>{
    expect(browserPointer({clientX:370,clientY:260},{left:10,top:20,width:720,height:480},{width:1440,height:960})).toEqual({x:720,y:480});
  });
  it("bounds edge clicks and rejects a hidden viewport",()=>{
    expect(browserPointer({clientX:2000,clientY:-2},{left:0,top:0,width:720,height:480},{width:1440,height:960})).toEqual({x:1439,y:0});
    expect(()=>browserPointer({clientX:0,clientY:0},{left:0,top:0,width:0,height:0},{width:1440,height:960})).toThrow();
  });
});
