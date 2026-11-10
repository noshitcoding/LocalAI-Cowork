import {createElement,Fragment,useMemo,useState,type ReactNode} from "react";
import snarkdown from "snarkdown";
import {useView} from "./view";
import "./chatEnhancements.css";

export function safeChatLink(value:string):string|null {
  try{const url=new URL(value);return ["https:","http:","mailto:"].includes(url.protocol)?url.href:null;}catch{return null;}
}
function CodeBlock({text,language}:{text:string;language:string}){
  const [status,setStatus]=useState(""),lang=useView(v=>v.language);
  return <div className="chat-code"><header><small>{language}</small><button type="button" onClick={()=>void navigator.clipboard.writeText(text).then(()=>setStatus(lang==="de"?"Kopiert":"Copied"),()=>setStatus(lang==="de"?"Kopieren fehlgeschlagen":"Copy failed"))}>{status||(lang==="de"?"Code kopieren":"Copy code")}</button></header><pre><code>{text}</code></pre></div>;
}
const allowed=new Set(["a","blockquote","br","code","del","em","h1","h2","h3","h4","h5","h6","hr","li","ol","p","pre","s","strong","table","tbody","td","th","thead","tr","ul"]);
function renderNode(node:Node,key:string):ReactNode{
  if(node.nodeType===3)return node.textContent;
  if(node.nodeType!==1)return null;
  const e=node as HTMLElement,tag=e.tagName.toLowerCase();
  if(["script","style","iframe","object","embed","img","svg","math"].includes(tag))return null;
  const children=Array.from(e.childNodes).map((child,i)=>renderNode(child,key+"-"+i));
  if(!allowed.has(tag))return createElement(Fragment,{key},children);
  if(tag==="pre")return <CodeBlock key={key} text={e.textContent??""} language={e.querySelector("code")?.className.replace(/^language-/,"")??""}/>;
  if(tag==="a"){
    const href=safeChatLink(e.getAttribute("href")??"");
    return href?<a key={key} href={href} target="_blank" rel="noopener noreferrer">{children}</a>:<span key={key}>{children}</span>;
  }
  const props:Record<string,unknown>={key};
  if(tag==="ol"){const start=Number(e.getAttribute("start"));if(Number.isInteger(start)&&start>0)props.start=start;}
  return createElement(tag,props,...(tag==="br"||tag==="hr"?[]:children));
}
function tableMarkdown(source:string){
  const lines=source.split(/\r?\n/),result:string[]=[];let fence=false;
  const cells=(line:string)=>line.trim().replace(/^\|/,"").replace(/\|$/,"").split(/(?<!\\)\|/).map(c=>c.trim());
  for(let i=0;i<lines.length;i++){
    if(/^\s*(?:```|~~~)/.test(lines[i]))fence=!fence;
    const headers=cells(lines[i]),separator=cells(lines[i+1]??"");
    if(fence||headers.length<2||headers.length!==separator.length||!separator.every(c=>/^:?-+:?$/.test(c))){result.push(lines[i]);continue;}
    const rows:string[][]=[];i+=2;
    while(i<lines.length&&lines[i].includes("|")&&cells(lines[i]).length===headers.length){rows.push(cells(lines[i]));i++;}i--;
    result.push(`<table><thead><tr>${headers.map(c=>`<th>${snarkdown(c)}</th>`).join("")}</tr></thead><tbody>${rows.map(row=>`<tr>${row.map(c=>`<td>${snarkdown(c)}</td>`).join("")}</tr>`).join("")}</tbody></table>`);
  }return result.join("\n");
}
export function ChatMessageBody({content,markdown=true}:{content:string;markdown?:boolean}){
  const lang=useView(v=>v.language);
  const parts=useMemo(()=>{
    if(!markdown)return content;
    if(/^\s*[\[{]/.test(content)){
      try{
        const value:unknown=JSON.parse(content);
        if(value!==null&&typeof value==="object")return <CodeBlock text={content} language="json"/>;
      }catch{/* Ordinary prose and incomplete streamed JSON retain Markdown rendering. */}
    }
    if(typeof DOMParser==="undefined")return content;
    // Parse into an inert document, then construct React elements from an allowlist.
    // No raw HTML, event handlers, remote images or arbitrary attributes enter React.
    const html=snarkdown(tableMarkdown(content)).replace(/<img\b[^>]*>/gi,"");
    const doc=new DOMParser().parseFromString(html,"text/html");
    return Array.from(doc.body.childNodes).map((node,i)=>renderNode(node,"m"+i));
  },[content,markdown]);
  const thinking=markdown?content.match(/^\s*<think>([\s\S]*?)<\/think>\s*([\s\S]*)$/):null;
  if(thinking)return <><details className="chat-thinking"><summary>{lang==="de"?"Gedankengang":"Thinking"}</summary><ChatMessageBody content={thinking[1]} markdown={false}/></details><ChatMessageBody content={thinking[2]}/></>;
  return <div className={markdown?"chat-markdown":"chat-plaintext"}>{parts}</div>;
}

export function chatTranscriptText(title:string,messages:{role:string;content:string;created_at:number}[],format:"md"|"txt"){
  return (format==="md"?`# ${title}\n\n`:`${title}\n\n`)+messages.map(m=>(format==="md"?`## ${m.role}\n\n`:`${m.role}\n`)+m.content).join("\n\n")+"\n";
}
