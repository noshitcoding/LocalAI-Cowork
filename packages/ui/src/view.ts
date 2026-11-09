import {create} from "zustand";
import {persist} from "zustand/middleware";
export const useView=create(persist<{
 language:"de"|"en"; theme:"light"|"dark"; project:string; order:string;
 setLanguage:(value:"de"|"en")=>void; setTheme:(value:"light"|"dark")=>void;
 selectProject:(id:string)=>void; selectOrder:(id:string)=>void;
 selectLocation:(project:string,order:string)=>void;
}>((set)=>({language:"de",theme:"light",project:"",order:"",
 setLanguage:language=>set({language}),setTheme:theme=>set({theme}),
 selectProject:project=>set({project,order:""}),selectOrder:order=>set({order}),selectLocation:(project,order)=>set({project,order})}),
 {name:"localai-cowork-v2-view"}));
