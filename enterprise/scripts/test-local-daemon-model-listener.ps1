param(
    [Parameter(Mandatory = $true)][int]$Port,
    [ValidateSet('quick', 'stall', 'tool')][string]$Mode = 'quick',
    [int]$RequestCount = 1,
    [string]$LogPath = '',
    [string]$ToolName = '',
    [string]$ToolArgumentsBase64 = '',
    [string]$FinalContent = 'detached client completed'
)

$ErrorActionPreference = 'Stop'
$listener = [Net.HttpListener]::new()
$listener.Prefixes.Add("http://127.0.0.1:$Port/")
$listener.Start()
try {
    foreach ($requestNumber in 1..$RequestCount) {
        $context = $listener.GetContext()
        $reader = [IO.StreamReader]::new($context.Request.InputStream)
        $requestBody = $reader.ReadToEnd()
        $reader.Dispose()
        if (-not [string]::IsNullOrWhiteSpace($LogPath)) {
            [IO.File]::AppendAllText(
                $LogPath,
                "request=$requestNumber path=$($context.Request.RawUrl) body=$requestBody`n",
                [Text.UTF8Encoding]::new($false)
            )
        }
        if ($Mode -eq 'stall') {
            Start-Sleep -Seconds 300
            continue
        }
        if ($Mode -eq 'tool') {
            if ($requestNumber -eq 1) {
                if ([string]::IsNullOrWhiteSpace($ToolName) -or [string]::IsNullOrWhiteSpace($ToolArgumentsBase64)) {
                    throw 'tool mode requires ToolName and ToolArgumentsBase64'
                }
                $argumentsJson = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($ToolArgumentsBase64))
                $responseObject = @{
                    id = 'test-tool-chat'
                    object = 'chat.completion'
                    created = 0
                    model = 'test-model'
                    choices = @(@{
                        index = 0
                        message = @{
                            role = 'assistant'
                            content = $null
                            tool_calls = @(@{
                                id = 'test-tool-call'
                                type = 'function'
                                function = @{ name = $ToolName; arguments = $argumentsJson }
                            })
                        }
                        finish_reason = 'tool_calls'
                    })
                    usage = @{ prompt_tokens = 4; completion_tokens = 3 }
                }
            }
            else {
                $responseObject = @{
                    id = 'test-tool-chat'
                    object = 'chat.completion'
                    created = 0
                    model = 'test-model'
                    choices = @(@{
                        index = 0
                        message = @{ role = 'assistant'; content = $FinalContent; tool_calls = @() }
                        finish_reason = 'stop'
                    })
                    usage = @{ prompt_tokens = 4; completion_tokens = 3 }
                }
            }
            $body = [Text.Encoding]::UTF8.GetBytes(($responseObject | ConvertTo-Json -Compress -Depth 20))
            $context.Response.StatusCode = 200
            $context.Response.ContentType = 'application/json'
            $context.Response.ContentLength64 = $body.Length
            $context.Response.OutputStream.Write($body, 0, $body.Length)
            $context.Response.OutputStream.Close()
            continue
        }
        $streaming = $false
        try {
            $requestJson = $requestBody | ConvertFrom-Json
            $streaming = $requestJson.stream -eq $true
        }
        catch {
            $streaming = $false
        }
        if ($streaming) {
            $payload = @(
                'data: {"id":"test-chat","object":"chat.completion.chunk","created":0,"model":"test-model","choices":[{"index":0,"delta":{"role":"assistant","content":"detached client completed"},"finish_reason":null}]}',
                '',
                'data: {"id":"test-chat","object":"chat.completion.chunk","created":0,"model":"test-model","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":4,"completion_tokens":3}}',
                '',
                'data: [DONE]',
                ''
            ) -join "`n"
            $body = [Text.Encoding]::UTF8.GetBytes($payload)
        }
        else {
            $responseObject = @{
                id = 'test-chat'
                object = 'chat.completion'
                created = 0
                model = 'test-model'
                choices = @(@{
                    index = 0
                    message = @{ role = 'assistant'; content = $FinalContent; tool_calls = @() }
                    finish_reason = 'stop'
                })
                usage = @{ prompt_tokens = 4; completion_tokens = 3 }
            }
            $body = [Text.Encoding]::UTF8.GetBytes(($responseObject | ConvertTo-Json -Compress -Depth 10))
        }
        $context.Response.StatusCode = 200
        $context.Response.ContentType = if ($streaming) { 'text/event-stream' } else { 'application/json' }
        $context.Response.ContentLength64 = $body.Length
        $context.Response.OutputStream.Write($body, 0, $body.Length)
        $context.Response.OutputStream.Close()
    }
}
finally {
    $listener.Stop()
}
