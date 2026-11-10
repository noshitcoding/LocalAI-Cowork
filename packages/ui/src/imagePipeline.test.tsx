import {describe,expect,it} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {ImagePipelineEditor} from "./ImagePipelineEditor";
import {imageDraft,imageDraftTool,imageOperations,imageStep,initialImageFields,newImageDraft} from "./imagePipeline";

const source={folder:"workspace",path:"pictures/source.png",sha256:"a".repeat(64)};
describe("image editing drafts",()=>{
  it("builds every supported operation without raw JSON input",()=>{
    expect(Object.keys(imageOperations)).toHaveLength(19);
    for(const op of Object.keys(imageOperations)){
      const fields=initialImageFields(op);
      if(op==="text")fields.text="Umlaute äöü";
      const step=imageStep(op,fields);
      expect(imageDraft({...newImageDraft(source),steps:[step]})?.steps).toEqual([step]);
    }
  });
  it("validates bounds, geometry and missing sizes before preparation",()=>{
    expect(()=>imageStep("crop",{...initialImageFields("crop"),"box.2":"0"})).toThrow();
    expect(()=>imageStep("resize",{...initialImageFields("resize"),width:"",height:""})).toThrow();
    expect(()=>imageStep("resize",{...initialImageFields("resize"),width:"1.5"})).toThrow();
    expect(()=>imageStep("blur",{radius:"Infinity"})).toThrow();
    expect(()=>imageStep("overlay",{...initialImageFields("overlay"),input:"16"})).toThrow();
  });
  it("accepts database key ordering and rejects altered or unknown steps",()=>{
    const step=imageStep("resize",initialImageFields("resize"));
    const sorted=Object.fromEntries(Object.entries(step).sort(([a],[b])=>a.localeCompare(b)));
    const draft={...newImageDraft(source),steps:[sorted]};
    expect(imageDraft(draft)?.steps).toEqual([step]);
    expect(()=>imageDraft({...draft,steps:[{...step,unexpected:true}]})).toThrow();
    expect(()=>imageDraft({...draft,steps:[{op:"constructor"}]})).toThrow();
    expect(()=>imageDraft({...draft,version:999})).toThrow();
  });
  it("binds the original source hash and requires actual overlay inputs",()=>{
    const draft={...newImageDraft(source),steps:[imageStep("overlay",initialImageFields("overlay"))]};
    expect(()=>imageDraftTool(draft)).toThrow("image_overlay_missing");
    const tool=imageDraftTool({...draft,overlays:["pictures/logo.png"]});
    expect(tool.arguments).toMatchObject({input:{folder_id:"workspace",path:source.path,expected_sha256:source.sha256},overlays:[{folder_id:"workspace",path:"pictures/logo.png"}],preview:true});
    expect(()=>imageDraftTool({...draft,output:source.path})).toThrow("image_output_invalid");
  });
  it("locks all controls in read-only mode and detects a changed source version",()=>{
    const props={source,value:newImageDraft(source),onChange:()=>{},locked:true,prepare:async()=>{throw new Error("must not dispatch");}};
    expect(renderToStaticMarkup(<ImagePipelineEditor {...props}/>)).toContain('<fieldset disabled="">');
    const html=renderToStaticMarkup(<ImagePipelineEditor {...props} source={{...source,sha256:"b".repeat(64)}}/>);
    expect(html).toContain('role="status"');
    expect(html).not.toContain('<fieldset');
  });
});
