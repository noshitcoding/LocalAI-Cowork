import type {ToolCall} from "@cowork/client";

type NumberField={key:string;label:string;en:string;type:"number";min:number;max:number;initial:number;integer?:boolean;optional?:boolean};
type TextField={key:string;label:string;en:string;type:"text";initial:string;maxLength?:number;optional?:boolean};
type ChoiceField={key:string;label:string;en:string;type:"choice";initial:string;choices:string[]};
type FlagField={key:string;label:string;en:string;type:"flag";initial:boolean};
export type ImageField=NumberField|TextField|ChoiceField|FlagField;
export type ImageStep=Record<string,unknown>&{op:string};
export type ImageDraft={version:1;source:{folder:string;path:string;sha256:string};steps:ImageStep[];output:string;quality:number;keepExif:boolean;overlays:string[]};
const n=(key:string,label:string,en:string,initial:number,min=0,max=100000,integer=true,optional=false):NumberField=>({key,label,en,type:"number",initial,min,max,integer,optional});
const text=(key:string,label:string,en:string,initial:string,optional=false,maxLength=100):TextField=>({key,label,en,type:"text",initial,optional,maxLength});
const choice=(key:string,label:string,en:string,initial:string,choices:string[]):ChoiceField=>({key,label,en,type:"choice",initial,choices});
const flag=(key:string,label:string,en:string,initial:boolean):FlagField=>({key,label,en,type:"flag",initial});
const box=(relative=false)=>[
  n("box.0","Links","Left",relative?0.1:0,0,relative?1:100000,!relative),n("box.1","Oben","Top",relative?0.1:0,0,relative?1:100000,!relative),
  n("box.2","Rechts","Right",relative?0.9:100,0,relative?1:100000,!relative),n("box.3","Unten","Bottom",relative?0.9:100,0,relative?1:100000,!relative),
];
const xy=()=>[n("x","X","X",10,-100000),n("y","Y","Y",10,-100000)];
const color=()=>text("color","Farbe","Color","#FF0000");
const shape=()=>[...box(),color(),n("width","Linienstärke","Stroke width",4,0,500),text("fill","Füllfarbe (optional)","Fill color (optional)","",true)];
export const imageOperations:Record<string,{de:string;en:string;fields:ImageField[]}>= {
  crop:{de:"Zuschneiden (Pixel)",en:"Crop (pixels)",fields:box()},
  crop_relative:{de:"Zuschneiden (Anteile 0–1)",en:"Crop (fractions 0–1)",fields:box(true)},
  resize:{de:"Größe ändern",en:"Resize",fields:[n("width","Breite","Width",800,1,100000,true,true),n("height","Höhe (optional)","Height (optional)",600,1,100000,true,true),choice("fit","Einpassung","Fit","contain",["contain","cover","stretch"])]},
  thumbnail:{de:"Vorschaubild",en:"Thumbnail",fields:[n("max_edge","Maximale Kantenlänge","Maximum edge",512,1)]},
  rotate:{de:"Drehen",en:"Rotate",fields:[n("degrees","Grad gegen den Uhrzeigersinn","Degrees counterclockwise",90,-360,360,false),flag("expand","Bildfläche erweitern","Expand canvas",true),text("fill","Hintergrund (optional)","Background (optional)","",true)]},
  flip:{de:"Spiegeln",en:"Flip",fields:[choice("direction","Richtung","Direction","horizontal",["horizontal","vertical"])]},
  auto_orient:{de:"EXIF-Ausrichtung anwenden",en:"Apply EXIF orientation",fields:[]},
  adjust:{de:"Helligkeit und Farben",en:"Brightness and colors",fields:[n("brightness","Helligkeit","Brightness",1,0,5,false),n("contrast","Kontrast","Contrast",1,0,5,false),n("saturation","Sättigung","Saturation",1,0,5,false),n("sharpness","Schärfe","Sharpness",1,0,5,false)]},
  grayscale:{de:"Graustufen",en:"Grayscale",fields:[]},
  blur:{de:"Weichzeichnen",en:"Blur",fields:[n("radius","Radius","Radius",2,0,100,false)]},
  sharpen:{de:"Nachschärfen",en:"Sharpen",fields:[n("amount","Stärke","Amount",1,0,5,false)]},
  text:{de:"Beschriften",en:"Add text",fields:[text("text","Beschriftung","Text","",false,2000),...xy(),n("size","Schriftgröße","Font size",32,4,2000),color(),text("background","Texthintergrund (optional)","Text background (optional)","",true),choice("anchor","Anker","Anchor","top_left",["top_left","top","top_right","left","center","right","bottom_left","bottom","bottom_right"]),n("padding","Textabstand","Text padding",8,0,1000)]},
  arrow:{de:"Pfeil",en:"Arrow",fields:[n("from.0","Start X","Start X",10),n("from.1","Start Y","Start Y",10),n("to.0","Ende X","End X",100),n("to.1","Ende Y","End Y",100),color(),n("width","Linienstärke","Stroke width",4,1,500)]},
  rectangle:{de:"Rechteck",en:"Rectangle",fields:shape()},
  ellipse:{de:"Ellipse",en:"Ellipse",fields:shape()},
  pad:{de:"Bildfläche vergrößern",en:"Pad canvas",fields:[n("left","Links","Left",20,0,10000),n("top","Oben","Top",20,0,10000),n("right","Rechts","Right",20,0,10000),n("bottom","Unten","Bottom",20,0,10000),color()]},
  border:{de:"Rahmen",en:"Border",fields:[n("width","Rahmenbreite","Border width",10,1,10000),color()]},
  overlay:{de:"Bild überlagern",en:"Overlay image",fields:[n("input","Nummer des Begleitbildes","Overlay image number",1,1,8),...xy(),n("opacity","Deckkraft","Opacity",1,0,1,false),n("scale","Skalierung","Scale",1,.01,20,false)]},
  convert:{de:"Farbmodus",en:"Color mode",fields:[choice("mode","Modus","Mode","RGBA",["RGB","RGBA","L","LA","P","1","CMYK"])]},
};

export function initialImageFields(op:string):Record<string,string|boolean>{
  return Object.fromEntries(imageOperations[op].fields.map(field=>[field.key,field.type==="flag"?field.initial:field.type==="number"&&field.optional&&field.key==="height"?"":String(field.initial)]));
}
export function imageStep(op:string,values:Record<string,string|boolean>):ImageStep{
  if(!Object.hasOwn(imageOperations,op))throw new Error("image_operation_invalid");
  const step:ImageStep={op};
  for(const field of imageOperations[op].fields){
    const raw=values[field.key];let value:unknown=raw;
    if((field.type==="number"||field.type==="text")&&field.optional&&raw==="")continue;
    if(field.type==="number"){
      value=typeof raw==="string"&&raw.trim()!==""?Number(raw):NaN;
      if(typeof value!=="number"||!Number.isFinite(value)||value<field.min||value>field.max||field.integer&&!Number.isInteger(value))throw new Error(field.label+": "+field.min+"…"+field.max);
    }else if(field.type==="choice"){
      if(typeof value!=="string"||!field.choices.includes(value))throw new Error(field.label);
    }else if(field.type==="flag"){
      if(typeof value!=="boolean")throw new Error(field.label);
    }else if(typeof value!=="string"||!value.trim()||value.length>(field.maxLength??100))throw new Error(field.label);
    const [key,index]=field.key.split(".");
    if(index!==undefined){const array=(step[key]??=[]) as unknown[];array[Number(index)]=value;}else step[key]=value;
  }
  if(Array.isArray(step.box)&&(Number(step.box[2])<=Number(step.box[0])||Number(step.box[3])<=Number(step.box[1])))throw new Error("crop_box_invalid");
  if(op==="resize"&&!step.width&&!step.height)throw new Error("image_size_required");
  if(op==="pad"&&![step.left,step.top,step.right,step.bottom].some(Boolean))throw new Error("image_padding_required");
  if(op==="arrow"&&JSON.stringify(step.from)===JSON.stringify(step.to))throw new Error("image_arrow_length_required");
  return step;
}

export function newImageDraft(source:ImageDraft["source"]):ImageDraft{
  return {version:1,source,steps:[],output:source.path.replace(/\.[^/.]+$/,"-edited.png"),quality:90,keepExif:false,overlays:[]};
}
export function imageDraft(value:unknown):ImageDraft|null{
  if(value===undefined||value===null)return null;
  if(typeof value!=="object"||Array.isArray(value))throw new Error("image_draft_invalid");
  const candidate=value as ImageDraft,source=candidate.source;
  if(candidate.version!==1||!source||typeof source.folder!=="string"||!source.folder||typeof source.path!=="string"||!source.path||typeof source.sha256!=="string"||! /^[a-f0-9]{64}$/.test(source.sha256)||!Array.isArray(candidate.steps)||candidate.steps.length>50||typeof candidate.output!=="string"||candidate.output.length>4096||!Number.isInteger(candidate.quality)||candidate.quality<1||candidate.quality>100||typeof candidate.keepExif!=="boolean"||!Array.isArray(candidate.overlays)||candidate.overlays.filter(Boolean).length>8||candidate.overlays.length>100||candidate.overlays.some(path=>typeof path!=="string"||path.length>4096))throw new Error("image_draft_invalid");
  for(const step of candidate.steps){
    if(!step||typeof step!=="object"||!Object.hasOwn(imageOperations,step.op))throw new Error("image_draft_invalid");
    const fields:Record<string,string|boolean>={};
    for(const field of imageOperations[step.op].fields){const [key,index]=field.key.split(".");const entry=index===undefined?step[key]:(step[key] as unknown[]|undefined)?.[Number(index)];fields[field.key]=field.type==="flag"?entry as boolean:entry===undefined?"":String(entry);}
    const expected=imageStep(step.op,fields);
    if(Object.keys(expected).length!==Object.keys(step).length||Object.keys(expected).some(key=>JSON.stringify(expected[key])!==JSON.stringify(step[key])))throw new Error("image_draft_invalid");
  }
  return candidate;
}
export function imageDraftTool(draft:ImageDraft):ToolCall{
  imageDraft(draft);
  if(!draft.steps.length||! /\.(png|jpe?g|webp|gif|bmp|tiff?)$/i.test(draft.output)||draft.output===draft.source.path)throw new Error("image_output_invalid");
  if(draft.steps.some(step=>step.op==="overlay"&&Number(step.input)>draft.overlays.length))throw new Error("image_overlay_missing");
  return {id:"ui",name:"image_edit",arguments:{input:{folder_id:draft.source.folder,path:draft.source.path,expected_sha256:draft.source.sha256},output:{path:draft.output},steps:draft.steps,overlays:draft.overlays.map(path=>({folder_id:draft.source.folder,path})),quality:draft.quality,keep_exif:draft.keepExif,preview:true}};
}
