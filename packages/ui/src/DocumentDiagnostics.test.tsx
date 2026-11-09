import {describe,expect,it} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {DocumentDiagnostics} from "./DocumentDiagnostics";

describe("visible document validation",()=>{
  it("shows converter failure messages without requiring raw result inspection",()=>{
    const html=renderToStaticMarkup(<DocumentDiagnostics engine={{error:"invalid_request",message:"LibreOffice could not convert this file"}}/>);
    expect(html).toContain('role="alert"');
    expect(html).toContain("LibreOffice could not convert this file");
  });
  it("makes spreadsheet error results visible even when the file was generated",()=>{
    const html=renderToStaticMarkup(<DocumentDiagnostics engine={{qa:{calculation:{status:"errors",formula_cells:4,missing_values:0,error_values:1}}}}/>);
    expect(html).toContain('role="alert"');
    expect(html).toContain("Formel-Fehlerwerte");
    expect(html).toContain("1 Fehlerwerte");
    expect(html).not.toContain("wurden mit LibreOffice neu berechnet");
  });
  it("distinguishes unverified inspection caches from real recalculation",()=>{
    expect(renderToStaticMarkup(<DocumentDiagnostics engine={{result:{calculation:{status:"unverified",formula_cells:2,missing_values:0,error_values:0}}}}/>)).toContain("ungeprüft");
    expect(renderToStaticMarkup(<DocumentDiagnostics engine={{qa:{calculation:{status:"recalculated",formula_cells:2,missing_values:0,error_values:0}}}}/>)).toContain("wurden mit LibreOffice neu berechnet");
  });
  it("escapes diagnostic text and shows source file and line",()=>{
    const html=renderToStaticMarkup(<DocumentDiagnostics engine={{diagnostics:[null,{severity:"error",file:"main.tex",line:12,message:"<script>unsafe</script>"}]}}/>);
    expect(html).toContain("main.tex:12");
    expect(html).not.toContain("<script>");
    expect(html).toContain("&lt;script&gt;");
  });
});
