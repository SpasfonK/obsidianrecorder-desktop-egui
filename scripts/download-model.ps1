# Télécharge le modèle Whisper recommandé (large-v3-turbo, quantifié q5_0,
# ~574 Mo) DANS LE MÊME DOSSIER QUE L'EXÉCUTABLE, pour une distribution
# portable : le .exe et le .bin se déplacent ensemble, sans rien écrire dans le
# profil utilisateur (voir src/paths.rs::portable_dir et src/stt/model.rs).
#
# Aucune dépendance : Invoke-WebRequest fait partie de PowerShell natif sous
# Windows (5.1 comme 7+), exactement dans l'esprit "zéro dépendance" du
# projet — ce script n'installe rien, il place un seul fichier au bon endroit.
#
# Usage :
#   .\scripts\download-model.ps1                 # dossier portable, sinon target\release puis target\debug
#   .\scripts\download-model.ps1 -DestinationDir "D:\ObsidianRecorder"

param(
    [string]$DestinationDir = ""
)

$ErrorActionPreference = "Stop"

$ModelUrl  = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin"
$ModelName = "ggml-large-v3-turbo-q5_0.bin"
$ExeName   = "obsidianrecorder-desktop.exe"

# Où placer le modèle ? Par défaut, le dossier qui contient l'exécutable :
#   - le dossier du script lui-même (cas de l'archive portable, où le .exe
#     et ce script sont extraits côte à côte) ;
#   - sinon target\release puis target\debug (cas du dépôt de développement) ;
#   - sinon le dossier courant, en prévenant l'utilisateur.
if ([string]::IsNullOrWhiteSpace($DestinationDir)) {
    $RepoRoot   = Split-Path -Parent $PSScriptRoot
    $Candidates = @(
        $PSScriptRoot,
        (Join-Path $RepoRoot "target\release"),
        (Join-Path $RepoRoot "target\debug")
    )

    $DestinationDir = $null
    foreach ($Candidate in $Candidates) {
        if (Test-Path (Join-Path $Candidate $ExeName)) {
            $DestinationDir = $Candidate
            break
        }
    }

    if (-not $DestinationDir) {
        $DestinationDir = (Get-Location).Path
        Write-Host "Exécutable introuvable — le modèle sera placé dans le dossier courant."
        Write-Host "Utilisez -DestinationDir pour cibler le dossier de l'application."
    }
}

New-Item -ItemType Directory -Force -Path $DestinationDir | Out-Null
$ModelPath = Join-Path $DestinationDir $ModelName

if (Test-Path $ModelPath) {
    Write-Host "Modèle déjà présent : $ModelPath"
    exit 0
}

Write-Host "Téléchargement du modèle Whisper large-v3-turbo (q5_0, ~574 Mo)..."
Write-Host "Source      : $ModelUrl"
Write-Host "Destination : $ModelPath"
Write-Host "(Cela peut prendre plusieurs minutes selon votre connexion.)"

$ProgressPreference = "Continue"
$TempPath = "$ModelPath.download"

try {
    Invoke-WebRequest -Uri $ModelUrl -OutFile $TempPath
    Move-Item -Force $TempPath $ModelPath
}
catch {
    if (Test-Path $TempPath) { Remove-Item -Force $TempPath }
    throw
}

Write-Host "Terminé : $ModelPath"
