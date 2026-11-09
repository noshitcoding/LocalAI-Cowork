import {describe,expect,it} from "vitest";
import {knowledgeImportSource,knowledgeImportTags,previewKnowledgeImport} from "./knowledgeImport";

describe("knowledge import preview",()=>{
  it("preserves all Unicode and line endings while previewing the exact persisted chunks",async()=>{
    const text=("Überprüfung 🧠 中文\r\n\r\nParagraph with evidence.\n").repeat(130);
    const source=await knowledgeImportSource("research.md",new TextEncoder().encode(text));
    const chunks=previewKnowledgeImport([source],{kind:"knowledge",tags:["research"],chunkCharacters:700});
    expect(chunks.length).toBeGreaterThan(2);
    expect(chunks.map(chunk=>chunk.document.content).join("")).toBe(text);
    for(const chunk of chunks){
      expect(Array.from(chunk.document.content).length).toBeLessThanOrEqual(700);
      expect(chunk.document.content).not.toMatch(/[\uD800-\uDBFF]$|^[\uDC00-\uDFFF]/);
      expect(chunk.document.provenance.origin).toBe("imported");
      expect(chunk.document.provenance.sources[0]).toMatchObject({sha256:source.sha256,verification:"declared",source_uri:`urn:cowork:knowledge-import:${source.sha256}`});
      expect(chunk.document.content).toBe(Array.from(text).slice(chunk.start,chunk.end).join(""));
    }
    expect(chunks[0].firstLine).toBe(1);
    expect(chunks[1].firstLine).toBe(chunks[0].lastLine);
  });
  it("validates every source before import and binds its original byte digest",async()=>{
    await expect(knowledgeImportSource("binary.txt",new Uint8Array([0xc3,0x28]))).rejects.toThrow();
    await expect(knowledgeImportSource("empty.txt",new TextEncoder().encode(" \n"))).rejects.toThrow("knowledge_import_text_required");
    await expect(knowledgeImportSource("null.txt",new TextEncoder().encode("a\0b"))).rejects.toThrow("knowledge_import_text_required");
    const first=await knowledgeImportSource("same.md",new TextEncoder().encode("alpha"));
    const second=await knowledgeImportSource("same.md",new TextEncoder().encode("alpha\n"));
    expect(first.sha256).not.toBe(second.sha256);
    expect(previewKnowledgeImport([first,second],{kind:"memory",tags:[],chunkCharacters:500}).map(chunk=>chunk.document.kind)).toEqual(["memory","memory"]);
    expect(()=>knowledgeImportTags("Topic,topic")).toThrow("invalid_knowledge_tags");
    expect(()=>previewKnowledgeImport([first],{kind:"knowledge",tags:[],chunkCharacters:4001})).toThrow("invalid_knowledge_chunk_size");
  });
  it("enforces bounded batches and preserves a short whitespace tail within the selected limit",async()=>{
    const source=await knowledgeImportSource("tail.md",new TextEncoder().encode("x".repeat(500)+"\n\n"));
    const chunks=previewKnowledgeImport([source],{kind:"knowledge",tags:[],chunkCharacters:500});
    expect(chunks).toHaveLength(2);expect(chunks.map(chunk=>chunk.document.content).join("")).toBe(source.text);
    expect(chunks.every(chunk=>Array.from(chunk.document.content).length<=500&&chunk.document.content.trim())).toBe(true);
    const huge={...source,text:"x".repeat(500*201)};
    expect(()=>previewKnowledgeImport([huge],{kind:"knowledge",tags:[],chunkCharacters:500})).toThrow("knowledge_import_chunk_limit");
    expect(()=>previewKnowledgeImport(Array(21).fill(source),{kind:"knowledge",tags:[],chunkCharacters:500})).toThrow("knowledge_import_source_limit");
  });
});
