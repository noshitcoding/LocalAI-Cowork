import {describe,expect,it} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import type {FolderGrant} from "@cowork/client";
import {DocumentToolSummary,isDocumentTool,summarizeDocumentTool} from "./DocumentToolSummary";
import {de,type Key} from "./i18n";

const t=(key:Key)=>de[key];
const folders:FolderGrant[]=[
  {id:"work",name:"Arbeitsdateien",order_id:"order",project_id:"p",resource_id:"r1",revision:"1",writable:true},
  {id:"docs",name:"Berichte",order_id:null,project_id:"p",resource_id:"r2",revision:"1",writable:true},
];

describe("document tool approval summary",()=>{
  it("lists sources, the new output and content counts",()=>{
    const summary=summarizeDocumentTool("document_write",{operation:"edit",inputs:[{folder_id:"docs",path:"q3.docx",expected_sha256:"a".repeat(64)}],output:{path:"q3-v2.docx"},spec:{patches:[{},{}],format:"docx"}});
    expect(summary).toMatchObject({operation:"edit",inputs:[{folder_id:"docs",path:"q3.docx"}],output:{path:"q3-v2.docx"}});
    expect(summary.counts).toEqual(["patches: 2","format: docx"]);
    const html=renderToStaticMarkup(<DocumentToolSummary name="document_write" args={{operation:"create_pdf",output:{path:"brief.pdf"},spec:{markdown:"# Titel"}}} folders={folders} t={t}/>);
    expect(html).toContain("Arbeitsdateien · brief.pdf");
    expect(html).toContain(de.documentNoOverwrite);
    expect(html).toContain("# Titel");
  });
  it("shows image steps and overlays",()=>{
    const summary=summarizeDocumentTool("image_edit",{input:{folder_id:"docs",path:"a.jpg"},output:{folder_id:"docs",path:"a-small.png"},steps:[{op:"crop",box:[0,0,10,10]},{op:"resize",width:5}],overlays:[{folder_id:"docs",path:"logo.png"}]});
    expect(summary.steps).toEqual(["crop","resize"]);
    expect(summary.inputs.map(i=>i.path)).toEqual(["a.jpg","logo.png"]);
    const html=renderToStaticMarkup(<DocumentToolSummary name="image_edit" args={{input:{folder_id:"docs",path:"a.jpg"},output:{folder_id:"docs",path:"b.png"},steps:[{op:"grayscale"}]}} folders={folders} t={t}/>);
    expect(html).toContain("Berichte · b.png");
    expect(html).toContain("grayscale");
  });
  it("recognizes only the built-in document tools",()=>{
    expect(["document_read","document_write","image_edit"].every(isDocumentTool)).toBe(true);
    expect(isDocumentTool("office")).toBe(false);
  });
});
