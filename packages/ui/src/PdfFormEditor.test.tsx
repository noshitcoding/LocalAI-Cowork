import {describe,expect,it} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {PdfFormEditor,pdfFormFields} from "./PdfFormEditor";
describe("PDF form inspection",()=>{
  it("normalizes corrupt field choices and rejects incomplete fields",()=>{
    expect(pdfFormFields([null,{name:"x"},{name:"A",type:"/Ch",value:"",states:"invalid",options:["One",null]}])).toEqual([{name:"A",type:"/Ch",value:"",states:[],options:["One"]}]);
  });
  it("shows only supported fields and locks read-only form submission",()=>{
    const html=renderToStaticMarkup(<PdfFormEditor folder="f" path="form.pdf" sha256={"a".repeat(64)} fields={[{name:"Name",type:"/Tx",value:"Ada"},{name:"Signed",type:"/Sig",value:""}]} locked={true} prepare={async()=>{throw new Error("must not dispatch");}}/>);
    expect(html).toContain('<fieldset disabled="">');
    expect(html).toContain('value="Ada"');
    expect(html).not.toContain('value="Signed"');
  });
});
