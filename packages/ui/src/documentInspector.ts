import type {OfficeItem,OfficeModel} from "./officeDocuments";
const object=(value:unknown):Record<string,any>=>value&&typeof value==="object"&&!Array.isArray(value)?value as Record<string,any>:{};

/** Editable items are bound to the exact inspect response, not the currently selected file. */
export function documentOfficeModel(raw:unknown,input:string,expectedSha:string):OfficeModel|null{
  const data=object(raw);
  if(!["docx","xlsx","pptx"].includes(data.format)||!Array.isArray(data.items))return null;
  if(!/^[a-f0-9]{64}$/.test(expectedSha)||data.source_sha256!==expectedSha)throw new Error("document_inspection_integrity");
  const items:OfficeItem[]=[];
  for(const value of data.items){const item=object(value);
    if(data.format==="docx"&&item.kind==="paragraph"&&Number.isInteger(item.paragraph))items.push({paragraph:item.paragraph,text:String(item.text??""),truncated:!!item.truncated||String(item.text??"").endsWith("…")});
    if(data.format==="xlsx"&&typeof item.sheet==="string"&&typeof item.cell==="string")items.push({...item,truncated:!!item.truncated||typeof item.value==="string"&&item.value.endsWith("…")});
    if(data.format==="pptx"&&Number.isInteger(item.slide)&&Array.isArray(item.shapes))for(const value of item.shapes){const shape=object(value);if(Number.isInteger(shape.shape)&&typeof shape.text==="string")items.push({slide:item.slide,shape:shape.shape,name:shape.name,text:shape.text,truncated:!!shape.truncated||shape.text.endsWith("…")});}
  }
  return {format:data.format,input,source_sha256:expectedSha,items,total:data.total??data.items.length,offset:data.offset??0,next_offset:data.next_offset??null};
}
