$ErrorActionPreference = 'Stop'

$testId = [guid]::NewGuid().ToString('N')
$testRoot = Join-Path ([System.IO.Path]::GetTempPath()) "cowork-local-e2e-$testId"
$workspace = Join-Path $testRoot 'workspace'
$data = Join-Path $testRoot 'data'
New-Item -ItemType Directory -Path $workspace, $data -Force | Out-Null
$port = 18765
$modelJob = Start-Job -ArgumentList $port -ScriptBlock {
  param($port)
  $listener = [System.Net.HttpListener]::new()
  $listener.Prefixes.Add("http://127.0.0.1:$port/")
  $listener.Start()
  try {
    for ($i = 0; $i -lt 3; $i++) {
      $context = $listener.GetContext()
      $reader = [IO.StreamReader]::new($context.Request.InputStream)
      $null = $reader.ReadToEnd()
      $reader.Dispose()
      if ($i -eq 0) {
        $payload = @{
          choices = @(@{
            message = @{
              content = $null
              tool_calls = @(@{
                id = 'call_write'
                type = 'function'
                function = @{
                  name = 'Write'
                  arguments = '{"path":"hello.txt","content":"hello durable local runtime"}'
                }
              })
            }
            finish_reason = 'tool_calls'
          })
          usage = @{ prompt_tokens = 10; completion_tokens = 5 }
        }
      } elseif ($i -eq 1) {
        $payload = @{
          choices = @(@{
            message = @{
              content = $null
              tool_calls = @(@{
                id = 'call_read'
                type = 'function'
                function = @{
                  name = 'Read'
                  arguments = '{"path":"hello.txt"}'
                }
              })
            }
            finish_reason = 'tool_calls'
          })
          usage = @{ prompt_tokens = 15; completion_tokens = 5 }
        }
      } else {
        $payload = @{
          choices = @(@{
            message = @{
              content = 'Local task completed after writing and reading the file.'
              tool_calls = @()
            }
            finish_reason = 'stop'
          })
          usage = @{ prompt_tokens = 20; completion_tokens = 8 }
        }
      }
      $bytes = [Text.Encoding]::UTF8.GetBytes(($payload | ConvertTo-Json -Compress -Depth 20))
      $context.Response.StatusCode = 200
      $context.Response.ContentType = 'application/json'
      $context.Response.ContentLength64 = $bytes.Length
      $context.Response.OutputStream.Write($bytes, 0, $bytes.Length)
      $context.Response.Close()
    }
  } finally {
    $listener.Stop()
  }
}

$pipeName = "cowork-local-e2e-$testId"
$env:COWORK_DAEMON_DATA_DIR = $data
$env:COWORK_DAEMON_IPC_ENDPOINT = "\\.\pipe\$pipeName"
$env:COWORK_DAEMON_IPC_TOKEN = 'local-e2e-token-000000000000000000000000'
$deviceId = [guid]::NewGuid()
$env:COWORK_DAEMON_DEVICE_ID = $deviceId.ToString()
$env:COWORK_MODEL_BASE_URL = "http://127.0.0.1:$port/v1"
$env:COWORK_MODEL_NAME = 'fake-local-model'
$stdout = Join-Path $testRoot 'daemon.stdout.log'
$stderr = Join-Path $testRoot 'daemon.stderr.log'
$daemonProcess = $null

try {
  $binary = (Resolve-Path 'target/debug/cowork-local-daemon.exe').Path
  $daemonProcess = Start-Process -FilePath $binary -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput $stdout -RedirectStandardError $stderr
  $pipe = [IO.Pipes.NamedPipeClientStream]::new(
    '.',
    $pipeName,
    [IO.Pipes.PipeDirection]::InOut,
    [IO.Pipes.PipeOptions]::None
  )
  $pipe.Connect(15000)
  $writer = [IO.StreamWriter]::new($pipe, [Text.UTF8Encoding]::new($false))
  $writer.AutoFlush = $true
  $reader = [IO.StreamReader]::new($pipe, [Text.UTF8Encoding]::new($false))
  $sequence = 0

  function Invoke-Local([string]$method, $parameters) {
    $script:sequence++
    $request = @{
      id = $script:sequence
      token = 'local-e2e-token-000000000000000000000000'
      method = $method
      params = $parameters
    }
    $writer.WriteLine(($request | ConvertTo-Json -Compress -Depth 30))
    $line = $reader.ReadLine()
    if (-not $line) { throw 'daemon closed the pipe' }
    $response = $line | ConvertFrom-Json
    if ($response.error) {
      throw "$($response.error.code): $($response.error.message)"
    }
    return $response.result
  }

  $projectId = [guid]::NewGuid()
  $threadId = [guid]::NewGuid()
  $null = Invoke-Local 'projects.bind_workspace' @{
    project_id = $projectId.ToString()
    workspace_path = $workspace
  }
  $created = Invoke-Local 'runs.create' @{
    thread_id = $threadId.ToString()
    project_id = $projectId.ToString()
    project_revision = 1
    project_privacy = 'private_local'
    task = $null
    executor_target = @{ kind = 'personal_device'; device_id = $deviceId.ToString() }
    required_capabilities = @('files')
    input = @{ prompt = 'Create hello.txt, then read it back.' }
    model_profile_id = $null
    snapshot_id = $null
    idempotency_key = "e2e-$testId"
  }
  $deadline = (Get-Date).AddSeconds(40)
  do {
    Start-Sleep -Milliseconds 250
    $record = Invoke-Local 'runs.get' @{ run_id = $created.spec.id }
  } while ($record.state -notin @('completed', 'failed', 'interrupted', 'canceled') -and (Get-Date) -lt $deadline)

  $events = Invoke-Local 'runs.events' @{ run_id = $created.spec.id; after = 0 }
  $checkpoints = Invoke-Local 'runs.checkpoints' @{ run_id = $created.spec.id }
  if ($record.state -ne 'completed') { throw "run ended in $($record.state): $($record.error.message)" }
  if ((Get-Content -Raw (Join-Path $workspace 'hello.txt')) -ne 'hello durable local runtime') {
    throw 'workspace result does not match'
  }
  if ($checkpoints.Count -ne 2) { throw "expected two checkpoints, got $($checkpoints.Count)" }

  Write-Output "run_state=$($record.state)"
  Write-Output "result_content=$($record.result.content)"
  Write-Output "workspace_content=$(Get-Content -Raw (Join-Path $workspace 'hello.txt'))"
  Write-Output "event_kinds=$(($events.kind -join ','))"
  Write-Output "checkpoint_count=$($checkpoints.Count)"
  Write-Output "checkpoint_safe=$(($checkpoints.safe_to_resume -join ','))"
  $pipe.Dispose()
} catch {
  if (Test-Path $stderr) { Get-Content $stderr }
  throw
} finally {
  if ($daemonProcess -and -not $daemonProcess.HasExited) {
    Stop-Process -Id $daemonProcess.Id -Force
  }
  if ($modelJob) {
    Stop-Job $modelJob -ErrorAction SilentlyContinue
    Remove-Job $modelJob -Force -ErrorAction SilentlyContinue
  }
  $resolvedRoot = [IO.Path]::GetFullPath($testRoot)
  $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
  if ($resolvedRoot.StartsWith($tempRoot) -and (Split-Path $resolvedRoot -Leaf).StartsWith('cowork-local-e2e-')) {
    Remove-Item -LiteralPath $resolvedRoot -Recurse -Force -ErrorAction SilentlyContinue
  }
}
