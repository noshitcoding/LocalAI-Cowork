param(
    [Parameter(Mandatory = $true)][string]$SourceRoot,
    [Parameter(Mandatory = $true)][string]$OutputRoot
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Release-Com($value) {
    if ($null -ne $value -and [Runtime.InteropServices.Marshal]::IsComObject($value)) {
        [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($value)
    }
}

New-Item -ItemType Directory -Path $OutputRoot -Force | Out-Null

$word = New-Object -ComObject Word.Application
try {
    $word.Visible = $false
    $word.DisplayAlerts = 0
    $word.AutomationSecurity = 3
    $document = $word.Documents.Open((Join-Path $SourceRoot 'source.docx'), $false, $true)
    if ($null -eq $document) { throw 'Word returned no document for the OOXML fixture' }
    $document.SaveAs2((Join-Path $OutputRoot 'legacy.doc'), 0)
    $document.Close($false)
    Release-Com $document
}
finally {
    $word.Quit()
    Release-Com $word
}

$excel = New-Object -ComObject Excel.Application
try {
    $excel.Visible = $false
    $excel.DisplayAlerts = $false
    $excel.AutomationSecurity = 3
    $workbook = $excel.Workbooks.Open((Join-Path $SourceRoot 'source.xlsx'), 0, $true)
    if ($null -eq $workbook) { throw 'Excel returned no workbook for the OOXML fixture' }
    $workbook.SaveAs((Join-Path $OutputRoot 'legacy.xls'), 56)
    $workbook.Close($false)
    Release-Com $workbook
}
finally {
    $excel.Quit()
    Release-Com $excel
}

$powerPoint = New-Object -ComObject PowerPoint.Application
try {
    $powerPoint.AutomationSecurity = 3
    $presentation = $powerPoint.Presentations.Open((Join-Path $SourceRoot 'source.pptx'), 0, 0, 0)
    if ($null -eq $presentation) { throw 'PowerPoint returned no presentation for the OOXML fixture' }
    $presentation.SaveAs((Join-Path $OutputRoot 'legacy.ppt'), 1)
    $presentation.Close()
    Release-Com $presentation
}
finally {
    $powerPoint.Quit()
    Release-Com $powerPoint
}

Write-Output 'legacy_office_fixtures=ok'
