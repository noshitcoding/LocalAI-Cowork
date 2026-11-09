import {describe,expect,it} from "vitest";
import type {Artifact} from "@cowork/client";
import {artifactDocument,artifactVersions,documentKind,verifyDocument} from "./documentFiles";
async function artifact(content:string,name="report.md"):Promise<Artifact>{
  const bytes=new TextEncoder().encode(content),sha256=Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256",bytes))).map(value=>value.toString(16).padStart(2,"0")).join("");
  return {id:"new",run_id:"run",order_id:"chat",name,media_type:"text/markdown",content,size:bytes.length,sha256,created_at:2};
}
describe("verified document workbench",()=>{
  it("checks original UTF-8 bytes before preview or saving",async()=>{
    const value=await artifact("# Überprüfung 🧠");
    const file=await artifactDocument(value);expect(await file.blob.text()).toBe(value.content);
    await expect(artifactDocument({...value,content:value.content+" changed"})).rejects.toThrow("download_integrity");
    await expect(verifyDocument({...file,sha256:"0".repeat(64)})).rejects.toThrow("download_integrity");
  });
  it("shows only actual same-name historical outputs from the same chat",async()=>{
    const value=await artifact("new"),old={...value,id:"old",created_at:1},otherChat={...value,id:"foreign",order_id:"other"},otherName={...value,id:"different",name:"different.md"};
    expect(artifactVersions(value,[old,otherChat,otherName,value]).map(item=>item.id)).toEqual(["new","old"]);
    expect(documentKind("report.docx")).toBe("office");expect(documentKind("report.pdf")).toBe("pdf");expect(documentKind("preview.html")).toBe("html");
    expect(documentKind("active.svg","image/svg+xml")).toBe("text");
  });
  it("recognizes TeX project sources and all raster engine formats with generic MIME types",()=>{
    for(const extension of ["tex","bib","sty","cls","bst"])expect(documentKind("project."+extension,"application/octet-stream")).toBe("text");
    for(const extension of ["png","jpg","webp","gif","bmp","tif","tiff"])expect(documentKind("image."+extension,"application/octet-stream")).toBe("image");
  });
});
