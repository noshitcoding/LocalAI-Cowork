$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$application = $env:COWORK_OFFICE_APP
$action = $env:COWORK_OFFICE_ACTION
$source = [System.IO.Path]::GetFullPath($env:COWORK_OFFICE_SOURCE)
$output = [System.IO.Path]::GetFullPath($env:COWORK_OFFICE_OUTPUT)
$previewOutput = $env:COWORK_OFFICE_PREVIEW_OUTPUT
$parameters = if ([string]::IsNullOrWhiteSpace($env:COWORK_OFFICE_PARAMETERS)) {
    [pscustomobject]@{}
} else {
    $env:COWORK_OFFICE_PARAMETERS | ConvertFrom-Json
}
$office = $null
$document = $null

function Get-ParameterValue([string]$Name, $Default = $null, [bool]$Required = $false) {
    $property = $parameters.PSObject.Properties[$Name]
    if ($null -ne $property -and $null -ne $property.Value) { return $property.Value }
    if ($Required) { throw "Missing Office action parameter: $Name" }
    return $Default
}

function Set-FontProperties($Font) {
    $bold = Get-ParameterValue 'bold'
    $italic = Get-ParameterValue 'italic'
    $size = Get-ParameterValue 'font_size'
    $name = Get-ParameterValue 'font_name'
    $color = Get-ParameterValue 'font_color'
    if ($null -ne $bold) { $Font.Bold = [int][bool]$bold }
    if ($null -ne $italic) { $Font.Italic = [int][bool]$italic }
    if ($null -ne $size) { $Font.Size = [double]$size }
    if ($null -ne $name) { $Font.Name = [string]$name }
    if ($null -ne $color) { $Font.Color = [int]$color }
}

function Save-WordDocument($Document, [string]$Path) {
    $Document.SaveAs2($Path, 16) # wdFormatDocumentDefault / DOCX
}

function Save-ExcelWorkbook($Workbook, [string]$Path) {
    $Workbook.SaveAs($Path, 51) # xlOpenXMLWorkbook / XLSX
}

function Save-PowerPointPresentation($Presentation, [string]$Path) {
    $Presentation.SaveAs($Path, 24) # ppSaveAsOpenXMLPresentation / PPTX
}

try {
    $readOnly = $action -eq 'export_pdf'
    switch ($application) {
        'word' {
            $office = New-Object -ComObject Word.Application
            $office.Visible = $true
            $office.DisplayAlerts = 0
            $office.AutomationSecurity = 3 # msoAutomationSecurityForceDisable
            $document = $office.Documents.Open($source, $false, $readOnly)
            switch ($action) {
                'export_pdf' {
                    $document.ExportAsFixedFormat($output, 17)
                }
                'replace_text' {
                    $oldText = [string](Get-ParameterValue 'old_text' $null $true)
                    $newText = [string](Get-ParameterValue 'new_text' $null $true)
                    $replaceAll = [bool](Get-ParameterValue 'replace_all' $true)
                    $replaceMode = if ($replaceAll) { 2 } else { 1 }
                    $find = $document.Content.Find
                    $find.ClearFormatting()
                    $find.Replacement.ClearFormatting()
                    [void]$find.Execute($oldText, $false, $false, $false, $false, $false, $true, 1, $false, $newText, $replaceMode)
                    Save-WordDocument $document $output
                }
                'word_append_paragraph' {
                    $text = [string](Get-ParameterValue 'text' $null $true)
                    $range = $document.Content
                    $range.Collapse(0) # wdCollapseEnd
                    $range.InsertAfter("`r`n$text")
                    Set-FontProperties $range.Font
                    Save-WordDocument $document $output
                }
                'word_format_text' {
                    $targetText = [string](Get-ParameterValue 'target_text' $null $true)
                    $range = $document.Content
                    $found = $range.Find.Execute($targetText)
                    if (-not $found) { throw "Word target_text was not found" }
                    Set-FontProperties $range.Font
                    Save-WordDocument $document $output
                }
                default { throw "Unsupported Word action: $action" }
            }
            if (-not [string]::IsNullOrWhiteSpace($previewOutput) -and $action -ne 'export_pdf') {
                $document.ExportAsFixedFormat($previewOutput, 17)
            }
        }
        'excel' {
            $office = New-Object -ComObject Excel.Application
            $office.Visible = $true
            $office.DisplayAlerts = $false
            $office.AutomationSecurity = 3
            $office.AskToUpdateLinks = $false
            $document = $office.Workbooks.Open($source, 0, $readOnly)
            $sheetName = Get-ParameterValue 'sheet'
            $sheet = if ($null -eq $sheetName) { $document.Worksheets.Item(1) } else { $document.Worksheets.Item([string]$sheetName) }
            switch ($action) {
                'export_pdf' {
                    $document.ExportAsFixedFormat(0, $output)
                }
                'replace_text' {
                    $oldText = [string](Get-ParameterValue 'old_text' $null $true)
                    $newText = [string](Get-ParameterValue 'new_text' $null $true)
                    $replaced = $false
                    foreach ($worksheet in $document.Worksheets) {
                        if ($worksheet.Cells.Replace($oldText, $newText, 2, 1, $false, $false, $false, $false)) { $replaced = $true }
                    }
                    if (-not $replaced) { throw "Excel old_text was not found" }
                    Save-ExcelWorkbook $document $output
                }
                'excel_set_cell' {
                    $cell = $sheet.Range([string](Get-ParameterValue 'cell' $null $true))
                    $formula = Get-ParameterValue 'formula'
                    if ($null -ne $formula) { $cell.Formula = [string]$formula } else { $cell.Value2 = Get-ParameterValue 'value' $null $true }
                    $numberFormat = Get-ParameterValue 'number_format'
                    if ($null -ne $numberFormat) { $cell.NumberFormat = [string]$numberFormat }
                    $office.CalculateFull()
                    Save-ExcelWorkbook $document $output
                }
                'excel_format_range' {
                    $range = $sheet.Range([string](Get-ParameterValue 'range' $null $true))
                    Set-FontProperties $range.Font
                    $numberFormat = Get-ParameterValue 'number_format'
                    if ($null -ne $numberFormat) { $range.NumberFormat = [string]$numberFormat }
                    Save-ExcelWorkbook $document $output
                }
                'excel_add_chart' {
                    $range = $sheet.Range([string](Get-ParameterValue 'range' $null $true))
                    $chartObject = $sheet.ChartObjects().Add(
                        [double](Get-ParameterValue 'left' 320),
                        [double](Get-ParameterValue 'top' 20),
                        [double](Get-ParameterValue 'width' 560),
                        [double](Get-ParameterValue 'height' 320)
                    )
                    $chart = $chartObject.Chart
                    $chart.SetSourceData($range)
                    $chart.ChartType = [int](Get-ParameterValue 'chart_type' 51) # clustered column
                    $title = Get-ParameterValue 'title'
                    if ($null -ne $title) { $chart.HasTitle = $true; $chart.ChartTitle.Text = [string]$title }
                    Save-ExcelWorkbook $document $output
                }
                default { throw "Unsupported Excel action: $action" }
            }
            if (-not [string]::IsNullOrWhiteSpace($previewOutput) -and $action -ne 'export_pdf') {
                $document.ExportAsFixedFormat(0, $previewOutput)
            }
        }
        'powerpoint' {
            $office = New-Object -ComObject PowerPoint.Application
            $office.Visible = -1
            $office.AutomationSecurity = 3
            $readOnlyFlag = if ($readOnly) { -1 } else { 0 }
            $document = $office.Presentations.Open($source, $readOnlyFlag, 0, -1)
            switch ($action) {
                'export_pdf' {
                    $document.SaveAs($output, 32)
                }
                'replace_text' {
                    $oldText = [string](Get-ParameterValue 'old_text' $null $true)
                    $newText = [string](Get-ParameterValue 'new_text' $null $true)
                    $replacements = 0
                    foreach ($slide in $document.Slides) {
                        foreach ($shape in $slide.Shapes) {
                            if ($shape.HasTextFrame -and $shape.TextFrame.HasText) {
                                $text = $shape.TextFrame.TextRange.Text
                                if ($text.Contains($oldText)) {
                                    $shape.TextFrame.TextRange.Text = $text.Replace($oldText, $newText)
                                    $replacements++
                                }
                            }
                        }
                    }
                    if ($replacements -eq 0) { throw "PowerPoint old_text was not found" }
                    Save-PowerPointPresentation $document $output
                }
                'powerpoint_add_slide' {
                    $layout = [int](Get-ParameterValue 'layout' 2)
                    $slide = $document.Slides.Add($document.Slides.Count + 1, $layout)
                    $title = Get-ParameterValue 'title'
                    $body = Get-ParameterValue 'body'
                    if ($null -ne $title -and $null -ne $slide.Shapes.Title) { $slide.Shapes.Title.TextFrame.TextRange.Text = [string]$title }
                    if ($null -ne $body -and $slide.Shapes.Placeholders.Count -ge 2) { $slide.Shapes.Placeholders.Item(2).TextFrame.TextRange.Text = [string]$body }
                    Save-PowerPointPresentation $document $output
                }
                'powerpoint_format_text' {
                    $targetText = [string](Get-ParameterValue 'target_text' $null $true)
                    $formatted = 0
                    foreach ($slide in $document.Slides) {
                        foreach ($shape in $slide.Shapes) {
                            if ($shape.HasTextFrame -and $shape.TextFrame.HasText -and $shape.TextFrame.TextRange.Text.Contains($targetText)) {
                                Set-FontProperties $shape.TextFrame.TextRange.Font
                                $formatted++
                            }
                        }
                    }
                    if ($formatted -eq 0) { throw "PowerPoint target_text was not found" }
                    Save-PowerPointPresentation $document $output
                }
                default { throw "Unsupported PowerPoint action: $action" }
            }
            if (-not [string]::IsNullOrWhiteSpace($previewOutput) -and $action -ne 'export_pdf') {
                $document.SaveAs($previewOutput, 32)
            }
        }
        default { throw "Unsupported Office application: $application" }
    }
    @{ application = $application; action = $action; output = $output; preview = $previewOutput } | ConvertTo-Json -Compress
}
finally {
    if ($null -ne $document) {
        try {
            if ($application -eq 'word') { $document.Close(0) }
            elseif ($application -eq 'excel') { $document.Close($false) }
            else { $document.Close() }
        } catch { [Console]::Error.WriteLine("Office document cleanup warning: $($_.Exception.Message)") }
        [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($document)
    }
    if ($null -ne $office) {
        try { $office.Quit() } catch { [Console]::Error.WriteLine("Office application cleanup warning: $($_.Exception.Message)") }
        [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($office)
    }
    try { Set-Clipboard -Value $null } catch {}
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
}
