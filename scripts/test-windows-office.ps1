param([string]$PythonPath = 'python')

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if ($env:OS -ne 'Windows_NT') {
  Write-Output 'windows_office_skipped=non_windows'
  exit 0
}

$workspace = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$adapter = Join-Path $workspace 'agents/cowork-device-agent/src/windows_office.ps1'
$testRoot = Join-Path ([IO.Path]::GetTempPath()) "cowork-windows-office-e2e-$([guid]::NewGuid().ToString('N'))"
$artifactRoot = Join-Path $testRoot 'artifacts'
New-Item -ItemType Directory -Path $artifactRoot -Force | Out-Null
$existingOfficeProcessIds = @(Get-Process WINWORD,EXCEL,POWERPNT -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Id)

function Release-Com($value) {
  if ($null -ne $value -and [Runtime.InteropServices.Marshal]::IsComObject($value)) {
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($value)
  }
}

function Invoke-OfficeAdapter(
  [string]$application,
  [string]$action,
  [string]$source,
  [string]$output,
  $parameters,
  [string]$preview = ''
) {
  $env:COWORK_OFFICE_APP = $application
  $env:COWORK_OFFICE_ACTION = $action
  $env:COWORK_OFFICE_SOURCE = $source
  $env:COWORK_OFFICE_OUTPUT = $output
  $env:COWORK_OFFICE_PREVIEW_OUTPUT = $preview
  $env:COWORK_OFFICE_PARAMETERS = $parameters | ConvertTo-Json -Compress -Depth 10
  $stdoutPath = Join-Path $testRoot "$application-$action.stdout.log"
  $stderrPath = Join-Path $testRoot "$application-$action.stderr.log"
  $process = Start-Process -FilePath 'powershell.exe' `
    -ArgumentList "-NoLogo -NoProfile -ExecutionPolicy Bypass -File `"$adapter`"" `
    -PassThru -WindowStyle Hidden -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath
  try {
    Wait-Process -Id $process.Id -Timeout 90 -ErrorAction Stop
  } catch {
    if (-not $process.HasExited) { Stop-Process -Id $process.Id -Force }
    Get-Process WINWORD,EXCEL,POWERPNT -ErrorAction SilentlyContinue |
      Where-Object { $_.Id -notin $existingOfficeProcessIds } |
      Stop-Process -Force -ErrorAction SilentlyContinue
    throw "Office adapter timed out for $application/$action; an unexpected dialog or first-run screen may require interactive setup"
  }
  $process.WaitForExit()
  $process.Refresh()
  $stdout = if (Test-Path -LiteralPath $stdoutPath) { @(Get-Content -LiteralPath $stdoutPath) } else { @() }
  $stderr = if (Test-Path -LiteralPath $stderrPath) { @(Get-Content -LiteralPath $stderrPath) } else { @() }
  if ($null -ne $process.ExitCode -and $process.ExitCode -ne 0) { throw "Office adapter failed for $application/$action (exit $($process.ExitCode))`: $stderr $stdout" }
  if (@($stdout).Count -eq 0) { throw "Office adapter returned no result for $application/$action`: $stderr" }
  return ($stdout | Select-Object -Last 1 | ConvertFrom-Json)
}

try {
  $wordSource = Join-Path $testRoot 'source.docx'
  $excelSource = Join-Path $testRoot 'source.xlsx'
  $powerPointSource = Join-Path $testRoot 'source.pptx'
  $fixtureScript = Join-Path $workspace 'scripts/create-office-fixtures.py'
  & $PythonPath $fixtureScript $testRoot
  if ($LASTEXITCODE -ne 0) {
    throw 'OOXML fixture creation failed; install python-docx, openpyxl and python-pptx or pass -PythonPath'
  }

  $wordOutput = Join-Path $artifactRoot 'word-edited.docx'
  $wordPreview = Join-Path $artifactRoot 'word-edited.pdf'
  $null = Invoke-OfficeAdapter 'word' 'replace_text' $wordSource $wordOutput `
    @{ old_text = 'old text'; new_text = 'new text'; replace_all = $true } $wordPreview

  $excelOutput = Join-Path $artifactRoot 'excel-edited.xlsx'
  $excelPreview = Join-Path $artifactRoot 'excel-edited.pdf'
  $null = Invoke-OfficeAdapter 'excel' 'excel_set_cell' $excelSource $excelOutput `
    @{ sheet = 'Data'; cell = 'C2'; formula = '=SUM(B1:B2)'; number_format = '0' } $excelPreview

  $chartOutput = Join-Path $artifactRoot 'excel-chart.xlsx'
  $null = Invoke-OfficeAdapter 'excel' 'excel_add_chart' $excelSource $chartOutput `
    @{ sheet = 'Data'; range = 'A1:B2'; title = 'E2E chart'; chart_type = 51 }

  $powerPointOutput = Join-Path $artifactRoot 'powerpoint-edited.pptx'
  $powerPointPreview = Join-Path $artifactRoot 'powerpoint-edited.pdf'
  $null = Invoke-OfficeAdapter 'powerpoint' 'powerpoint_add_slide' $powerPointSource $powerPointOutput `
    @{ layout = 2; title = 'Added by Open Cowork'; body = 'Managed Windows executor' } $powerPointPreview

  foreach ($path in @($wordOutput, $wordPreview, $excelOutput, $excelPreview, $chartOutput, $powerPointOutput, $powerPointPreview)) {
    if (-not (Test-Path -LiteralPath $path) -or (Get-Item -LiteralPath $path).Length -eq 0) {
      throw "Office adapter did not create $path"
    }
  }

  $verifyWord = New-Object -ComObject Word.Application
  try {
    $verifyDocument = $verifyWord.Documents.Open($wordOutput, $false, $true)
    if ($verifyDocument.Content.Text -notmatch 'new text') { throw 'Word replacement was not persisted' }
    $verifyDocument.Close(0)
    Release-Com $verifyDocument
  } finally {
    $verifyWord.Quit()
    Release-Com $verifyWord
  }

  $verifyExcel = New-Object -ComObject Excel.Application
  try {
    $verifyWorkbook = $verifyExcel.Workbooks.Open($excelOutput, 0, $true)
    $verifySheet = $verifyWorkbook.Worksheets.Item('Data')
    if ($verifySheet.Range('C2').Formula -ne '=SUM(B1:B2)') { throw 'Excel formula was not persisted' }
    $verifyWorkbook.Close($false)
    Release-Com $verifySheet
    Release-Com $verifyWorkbook
  } finally {
    $verifyExcel.Quit()
    Release-Com $verifyExcel
  }

  $verifyPowerPoint = New-Object -ComObject PowerPoint.Application
  try {
    $verifyPresentation = $verifyPowerPoint.Presentations.Open($powerPointOutput, -1, 0, 0)
    if ($verifyPresentation.Slides.Count -ne 2 -or $verifyPresentation.Slides.Item(2).Shapes.Title.TextFrame.TextRange.Text -ne 'Added by Open Cowork') {
      throw 'PowerPoint slide was not persisted'
    }
    $verifyPresentation.Close()
    Release-Com $verifyPresentation
  } finally {
    $verifyPowerPoint.Quit()
    Release-Com $verifyPowerPoint
  }

  Write-Output "word_output_bytes=$((Get-Item -LiteralPath $wordOutput).Length)"
  Write-Output "word_preview_bytes=$((Get-Item -LiteralPath $wordPreview).Length)"
  Write-Output "excel_output_bytes=$((Get-Item -LiteralPath $excelOutput).Length)"
  Write-Output "excel_chart_bytes=$((Get-Item -LiteralPath $chartOutput).Length)"
  Write-Output "powerpoint_output_bytes=$((Get-Item -LiteralPath $powerPointOutput).Length)"
  Write-Output "powerpoint_preview_bytes=$((Get-Item -LiteralPath $powerPointPreview).Length)"
} finally {
  Get-Process WINWORD,EXCEL,POWERPNT -ErrorAction SilentlyContinue |
    Where-Object { $_.Id -notin $existingOfficeProcessIds } |
    Stop-Process -Force -ErrorAction SilentlyContinue
  $resolvedRoot = [IO.Path]::GetFullPath($testRoot)
  $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
  if ($resolvedRoot.StartsWith($tempRoot) -and (Split-Path $resolvedRoot -Leaf).StartsWith('cowork-windows-office-e2e-')) {
    Remove-Item -LiteralPath $resolvedRoot -Recurse -Force -ErrorAction SilentlyContinue
  }
}
