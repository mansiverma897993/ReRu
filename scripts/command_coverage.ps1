param(
    [Parameter(Mandatory = $true)]
    [string]$RedisSource
)
$ErrorActionPreference = 'Stop'

$metadataDirectory = Join-Path $RedisSource 'src/commands'
if (-not (Test-Path -LiteralPath $metadataDirectory -PathType Container)) {
    throw "Redis command metadata not found at $metadataDirectory"
}

$repo = Split-Path -Parent $PSScriptRoot
$inventory = Join-Path $repo 'docs/COMPATIBILITY.md'
$areaPattern = '^\| (Connection|Strings/cache|Lists|Streams|Hashes|Sets|Sorted sets|Bitmaps|HyperLogLog|Geospatial|Keyspace|Transactions|Pub/Sub|Connection close|Authentication|Configuration|Observability|Server time|Persistence|Command introspection|Static Cluster) \|'
$implemented = @(
    Get-Content -LiteralPath $inventory |
        Where-Object { $_ -match $areaPattern } |
        ForEach-Object {
            $column = ($_ -split '\|')[2]
            [regex]::Matches($column, '`([^`]+)`') | ForEach-Object { $_.Groups[1].Value }
        } |
        Sort-Object -Unique
)

$reference = @(
    Get-ChildItem -LiteralPath $metadataDirectory -Filter '*.json' |
        ForEach-Object {
            $record = Get-Content -LiteralPath $_.FullName -Raw | ConvertFrom-Json
            foreach ($property in $record.PSObject.Properties) {
                $name = $property.Name
                $container = $property.Value.container
                if ($container) { "$container $name" } else { $name }
            }
        } |
        Sort-Object -Unique
)

$present = @($implemented | Where-Object { $_ -in $reference })
$unmapped = @($implemented | Where-Object { $_ -notin $reference })
$percentage = [math]::Round(100 * $present.Count / $reference.Count, 1)
Write-Output "Redis metadata entries: $($reference.Count)"
Write-Output "ReRu inventory entries: $($implemented.Count)"
Write-Output "Matching command names: $($present.Count) ($percentage%)"
if ($unmapped.Count -gt 0) {
    Write-Output "Inventory names absent from reference: $($unmapped -join ', ')"
}
Write-Output 'Name presence is not behavior compatibility or overall Redis completion.'
