
$ErrorActionPreference = "Stop"

# Project folder
$ProjectRoot = $PSScriptRoot
$ProjectName = Split-Path $ProjectRoot -Leaf
$ParentFolder = Split-Path $ProjectRoot -Parent

# Output file
$OutputPath = Join-Path $ParentFolder "$ProjectName`_source.txt"

# Allowed file extensions
$AllowedExtensions = @(
    ".rs", ".toml", ".ron", ".json",
    ".yaml", ".yml", ".wgsl", ".glsl",
    ".vert", ".frag", ".md", ".txt",
    ".ps1", ".bat", ".cmd", ".sh",
    ".xml", ".csv"
)

# Allowed files without extensions
$AllowedNames = @(
    "Cargo.lock",
    ".gitignore",
    ".gitattributes",
    "rust-toolchain"
)

# Directories to skip
$ExcludedDirectories = @(
    "target",
    ".git",
    ".idea",
    ".vscode",
    "node_modules",
    "vendor",
    "dist",
    "build",
    ".venv"
)

# Maximum file size: 2 MB
$MaxFileSize = 2MB

Write-Host "Scanning project..." -ForegroundColor Cyan

$Files = Get-ChildItem -LiteralPath $ProjectRoot -Recurse -File -Force |
    Where-Object {
        $File = $_

        $RelativePath = $File.FullName.Substring($ProjectRoot.Length)
        $PathParts = $RelativePath -split '[\\/]'

        $SkipDirectory = $false

        foreach ($Part in $PathParts) {
            if ($ExcludedDirectories -contains $Part) {
                $SkipDirectory = $true
                break
            }
        }

        $Allowed = (
            $AllowedExtensions -contains $File.Extension.ToLowerInvariant()
        ) -or (
            $AllowedNames -contains $File.Name
        )

        (-not $SkipDirectory) -and
        $Allowed -and
        ($File.Length -le $MaxFileSize)
    } |
    Sort-Object FullName

$Builder = New-Object System.Text.StringBuilder

[void]$Builder.AppendLine("========================================")
[void]$Builder.AppendLine("PLANETSIM PROJECT SOURCE")
[void]$Builder.AppendLine("Export date: $(Get-Date)")
[void]$Builder.AppendLine("Project: $ProjectRoot")
[void]$Builder.AppendLine("========================================")

$ExportedCount = 0

foreach ($File in $Files) {
    $RelativePath = $File.FullName.Substring($ProjectRoot.Length).TrimStart('\', '/')

    try {
        $Content = [System.IO.File]::ReadAllText(
            $File.FullName,
            [System.Text.Encoding]::UTF8
        )

        [void]$Builder.AppendLine("")
        [void]$Builder.AppendLine("========================================")
        [void]$Builder.AppendLine("FILE: $RelativePath")
        [void]$Builder.AppendLine("========================================")
        [void]$Builder.AppendLine($Content)

        $ExportedCount++

        Write-Host "[OK] $RelativePath" -ForegroundColor Green
    }
    catch {
        Write-Host "[ERROR] $RelativePath" -ForegroundColor Red
        Write-Host $_.Exception.Message
    }
}

[System.IO.File]::WriteAllText(
    $OutputPath,
    $Builder.ToString(),
    [System.Text.Encoding]::UTF8
)

Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "EXPORT COMPLETE" -ForegroundColor Green
Write-Host "Files exported: $ExportedCount"
Write-Host "Output: $OutputPath"
Write-Host "========================================" -ForegroundColor Cyan

Invoke-Item $OutputPath
