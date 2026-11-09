/** Select exact JSON bytes without rounding native integers through JS numbers. */
export function canonicalJsonValue(text:string,path:readonly string[]):string{
  if(new TextEncoder().encode(text).length>8*1024*1024||path.length>32)throw new Error("canonical_json_invalid");
  JSON.parse(text);
  const whitespace=(i:number)=>{while(i<text.length&&/\s/.test(text[i]))i++;return i;};
  function stringEnd(start:number):number{
    let i=start+1;
    while(i<text.length){if(text[i]==="\\"){i+=2;continue;}if(text[i]==='"')return i+1;i++;}
    throw new Error("canonical_json_invalid");
  }
  function end(start:number):number{
    const first=text[start];
    if(first==='"')return stringEnd(start);
    if(first!=="{"&&first!=="["){
      let i=start;while(i<text.length&&!/[\s,}\]]/.test(text[i]))i++;
      if(i===start)throw new Error("canonical_json_invalid");return i;
    }
    let i=start+1,depth=1;
    while(i<text.length){
      if(text[i]==='"'){i=stringEnd(i);continue;}
      if(text[i]==="{"||text[i]==="["){if(++depth>256)throw new Error("canonical_json_invalid");}
      else if(text[i]==="}"||text[i]==="]"){if(--depth===0)return i+1;}
      i++;
    }
    throw new Error("canonical_json_invalid");
  }
  let start=whitespace(0),limit=end(start);
  for(const key of path){
    if(text[start]!=="{")throw new Error("canonical_json_path_missing");
    let i=whitespace(start+1),chosen:readonly [number,number]|undefined;
    const seen=new Set<string>();
    while(i<limit&&text[i]!=="}"){
      if(text[i]!=='"')throw new Error("canonical_json_invalid");
      const keyEnd=stringEnd(i),name=JSON.parse(text.slice(i,keyEnd)) as string;
      if(seen.has(name))throw new Error("canonical_json_duplicate_key");seen.add(name);
      i=whitespace(keyEnd);if(text[i]!==":")throw new Error("canonical_json_invalid");
      const valueStart=whitespace(i+1),valueEnd=end(valueStart);
      if(name===key)chosen=[valueStart,valueEnd];
      i=whitespace(valueEnd);if(text[i]===",")i=whitespace(i+1);else if(text[i]!=="}")throw new Error("canonical_json_invalid");
    }
    if(!chosen)throw new Error("canonical_json_path_missing");[start,limit]=chosen;
  }
  return text.slice(start,limit);
}
