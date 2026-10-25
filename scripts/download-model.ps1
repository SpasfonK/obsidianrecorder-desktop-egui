# Télécharge le modèle Whisper recommandé (large-v3-turbo, quantifié q5_0,
# ~574 Mo) dans %LOCALAPPDATA%\ObsidianRECORDER\models\.
#
# Aucune dépendance : Invoke-WebRequest fait partie de PowerShell natif sous
# Windows (5.1 comme 7+), exactement dans l'esprit "zéro dépendance" du
# projet — ce script n'installe rien, il place un seul fichier au bon endroit.

$ErrorActionPreference = "Stop"

$ModelUrl  = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin"
$ModelDir  = Join-Path $env:LOCALAPPDATA "ObsidianRECORDER\models"
$ModelPath = Join-Path $ModelDir "ggml-large-v3-turbo-q5_0.bin"

if (Test-Path $ModelPath) {
    Write-Host "Modèle déjà présent : $ModelPath"
    exit 0
}

New-Item -ItemType Directory -Force -Path $ModelDir | Out-Null

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
