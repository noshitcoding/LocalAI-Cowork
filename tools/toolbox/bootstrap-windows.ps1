param([Parameter(Mandatory=$true)][string]$Archive,[Parameter(Mandatory=$true)][string]$Destination)
$ErrorActionPreference = 'Stop'
# Only expands the already SHA-256 checked official CPython embed archive.
# No install manager, package manager, registry, aliases, or machine PATH changes.
if (-not [IO.Path]::IsPathRooted($Archive) -or -not [IO.Path]::IsPathRooted($Destination)) { throw 'Absolute private paths required' }
Expand-Archive -LiteralPath $Archive -DestinationPath $Destination -Force
$configuration = Join-Path $Destination 'python313._pth'
[IO.File]::WriteAllText($configuration, "python313.zip`n.`nLib/site-packages`nimport site`n", [Text.UTF8Encoding]::new($false))
if (-not (Test-Path -LiteralPath (Join-Path $Destination 'python.exe') -PathType Leaf)) { throw 'Private Python was not extracted' }
