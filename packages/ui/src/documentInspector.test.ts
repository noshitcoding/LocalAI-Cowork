import {describe,expect,it} from "vitest";
import {documentOfficeModel} from "./documentInspector";
describe("document inspection binding",()=>{
  const sha="a".repeat(64);
  it("refuses inspection from a different source version",()=>{
    expect(()=>documentOfficeModel({format:"docx",source_sha256:"b".repeat(64),items:[]},"a.docx",sha)).toThrow("document_inspection_integrity");
  });
  it("keeps original paragraph and shape identities and prevents truncated replacement",()=>{
    const word=documentOfficeModel({format:"docx",source_sha256:sha,items:[{kind:"table",table:0},{kind:"paragraph",paragraph:7,text:"shortened…"}],total:2},"a.docx",sha)!;
    expect(word.items).toEqual([{paragraph:7,text:"shortened…",truncated:true}]);
    const slides=documentOfficeModel({format:"pptx",source_sha256:sha,items:[{slide:4,shapes:[{shape:12,text:"Title",name:"Heading"}]}]},"a.pptx",sha)!;
    expect(slides.items[0]).toMatchObject({slide:4,shape:12,text:"Title"});
  });
});
