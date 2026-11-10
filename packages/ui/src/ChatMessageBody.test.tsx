import {describe,it,expect} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {ChatMessageBody,safeChatLink,chatTranscriptText} from "./ChatMessageBody";
describe("chat display and export",()=>{
 it("renders structured workflow results as exact data without Markdown changing fields or arrays",()=>{
  const content='{"diagnostics":[],"diagnostics_truncated":false,"file":{"folder_id":"workspace:original","mime_type":"application/test","path":"report_one.docx"},"large_integer":9007199254740993}';
  const html=renderToStaticMarkup(<ChatMessageBody content={content}/>);
  expect(html).toContain('class="chat-code"');expect(html).toContain('<small>json</small>');
  expect(html).toContain('&quot;diagnostics&quot;:[]');expect(html).toContain('&quot;diagnostics_truncated&quot;:false');
  expect(html).toContain('folder_id');expect(html).toContain('mime_type');expect(html).toContain('report_one.docx');expect(html).toContain('9007199254740993');
 });
 it("keeps markup in JSON inert and leaves ordinary or incomplete content in its existing display",()=>{
  const json=renderToStaticMarkup(<ChatMessageBody content={'["<script>window.injected=true</script>","[label](javascript:alert(1))"]'}/>);
  expect(json).toContain('&lt;script&gt;');expect(json).not.toContain('<script>');expect(json).not.toContain('<a ');
  for(const content of ['Ordinary **message**','{"partial":', '[a Markdown link](https://example.com)'])expect(renderToStaticMarkup(<ChatMessageBody content={content}/>)).not.toContain('class="chat-code"');
 });
 it("allows ordinary links and rejects executable and local URLs",()=>{
  expect(safeChatLink("https://example.com/a?q=1")).toBe("https://example.com/a?q=1");
  for(const value of ["javascript:alert(1)","data:text/html,<script>alert(1)</script>","file:///etc/passwd","/relative","vbscript:msgbox(1)"])expect(safeChatLink(value)).toBeNull();
 });
 it("preserves exact message content and order in text and Markdown exports",()=>{
  const messages=[{role:"user",content:"Grüße 🙂\n`code`",created_at:1},{role:"assistant",content:"**Antwort**\n\nText",created_at:2}];
  expect(chatTranscriptText("Chat",messages,"md")).toBe("# Chat\n\n## user\n\nGrüße 🙂\n`code`\n\n## assistant\n\n**Antwort**\n\nText\n");
  expect(chatTranscriptText("Chat",messages,"txt")).toBe("Chat\n\nuser\nGrüße 🙂\n`code`\n\nassistant\n**Antwort**\n\nText\n");
 });
});
