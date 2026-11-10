// Typed display projections of immutable, explicitly selected native results.
import type {WorkflowDocumentOperation,WorkflowReviewEvidence,WorkflowReviewRequest} from "@cowork/client";
import {validDataValue} from "./workflowDataflow";
import {documentOutputType} from "./workflowDocumentTypes";

export type ReviewSourcePage={page:number;width:number;height:number;preview_sha256:string|null;raster_sha256:string;dpi:number;rotation:number;word_count:number};
export type ReviewSourceWord={page:number;index:number;line:number;text:string;text_truncated:boolean;x:number;y:number;width:number;height:number;score:number};
export type ReviewSourceOutput={source:{folder_id:string;path:string;sha256:string};text:string;truncated:boolean;offset:number;next_offset:number|null;pages?:ReviewSourcePage[];words?:ReviewSourceWord[];pages_truncated?:boolean;next_page?:number|null;diagnostics:{severity:string;message:string;file:string|null;line:number|null}[];diagnostics_truncated:boolean};
const digest=/^[a-f0-9]{64}$/;
const dimension=(n:number)=>Number.isSafeInteger(n)&&n>0&&n<=100_000;
export function reviewSourceOutput(evidence:WorkflowReviewEvidence):ReviewSourceOutput|null {
  const operation:WorkflowDocumentOperation=evidence.kind==="text"?{kind:"read_text",path_input:"path",sha256_input:"sha",offset:0}:{kind:"ocr",path_input:"path",sha256_input:"sha",languages:["eng"],pages:[],dpi:200,rotation:0,offset:0,max_words:100};
  if(!validDataValue(documentOutputType(operation),evidence.output))return null;
  const output=evidence.output as ReviewSourceOutput;
  if(!output.source.folder_id||!output.source.path||!digest.test(output.source.sha256)||output.offset<0||output.next_offset!==null&&output.next_offset<0)return null;
  if(evidence.kind==="ocr"){
    const pages=output.pages??[],words=output.words??[];
    if(new Set(pages.map(p=>p.page)).size!==pages.length||new Set(words.map(w=>`${w.page}:${w.index}`)).size!==words.length)return null;
    if(pages.some(p=>p.page<1||!dimension(p.width)||!dimension(p.height)||!digest.test(p.raster_sha256)||p.preview_sha256!==null&&!digest.test(p.preview_sha256)))return null;
    if(words.some(word=>{const page=pages.find(p=>p.page===word.page);return !page||word.index<0||word.line<0||word.x<0||word.y<0||word.width<=0||word.height<=0||word.x+word.width>page.width||word.y+word.height>page.height||word.score<0||word.score>100;}))return null;
  }
  return output;
}
export function sourceWordMatches(words:ReviewSourceWord[],query:string):ReviewSourceWord[] {
  const text=query.trim().toLocaleLowerCase();return text?words.filter(word=>word.text.toLocaleLowerCase().includes(text)):words;
}
export function validReviewEvidence(review:WorkflowReviewRequest):boolean {
  const selected=review.form.source_steps??[],evidence=review.evidence??[];
  if(selected.length>4||selected.length!==evidence.length||new Set(selected).size!==selected.length)return false;
  let bytes=0;
  try{
    return evidence.every((item,index)=>{
      bytes+=new TextEncoder().encode(JSON.stringify(item.output)).byteLength;
      return item.step_id===selected[index]&&["text","ocr"].includes(item.kind)&&!!item.name.trim()&&new TextEncoder().encode(item.name).byteLength<=200&&bytes<=128_000&&reviewSourceOutput(item)!==null;
    });
  }catch{return false;}
}
