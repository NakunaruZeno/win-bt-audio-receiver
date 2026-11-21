$ErrorActionPreference = "Stop"

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$javaSrc = Join-Path $scriptDir "src\com\aethersora\Agent.java"
$outDir = Join-Path $scriptDir "out"
$dexOut = Join-Path $scriptDir "aether-agent.dex"

# Locate javac (PATH or Android Studio JBR)
$javac = (Get-Command javac -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source -First 1)
if (-not $javac) {
    $jbrJavac = "C:\Program Files\Android\Android Studio\jbr\bin\javac.exe"
    if (Test-Path $jbrJavac) { $javac = $jbrJavac }
}
if (-not $javac) { throw "javac.exe not found. Install a JDK or Android Studio." }

$javaBinDir = Split-Path -Parent $javac
$env:PATH = "$javaBinDir;$env:PATH"

# Locate android.jar and d8.bat in %LOCALAPPDATA%\Android\Sdk
$sdkRoot = "$env:LOCALAPPDATA\Android\Sdk"
$androidJar = Get-ChildItem -Path "$sdkRoot\platforms" -Recurse -Filter "android.jar" -ErrorAction SilentlyContinue |
    Sort-Object FullName -Descending | Select-Object -ExpandProperty FullName -First 1
$d8Bat = Get-ChildItem -Path "$sdkRoot\build-tools" -Recurse -Filter "d8.bat" -ErrorAction SilentlyContinue |
    Sort-Object FullName -Descending | Select-Object -ExpandProperty FullName -First 1

if (-not $androidJar) { throw "android.jar not found under $sdkRoot\platforms" }
if (-not $d8Bat) { throw "d8.bat not found under $sdkRoot\build-tools" }

if (Test-Path $outDir) { Remove-Item -Recurse -Force $outDir }
New-Item -ItemType Directory -Path $outDir | Out-Null

Write-Host "[1/2] Compiling Agent.java with $javac..."
& $javac --release 8 -cp $androidJar -d $outDir $javaSrc
if ($LASTEXITCODE -ne 0) { throw "javac compilation failed" }

$classFiles = Get-ChildItem -Path $outDir -Recurse -Filter "*.class" | Select-Object -ExpandProperty FullName
Write-Host "[2/2] Packaging DEX with $d8Bat..."
& $d8Bat --release --min-api 26 --output $outDir $classFiles
if ($LASTEXITCODE -ne 0) { throw "d8 packaging failed" }

$classesDex = Join-Path $outDir "classes.dex"
Copy-Item -Force $classesDex $dexOut
Remove-Item -Recurse -Force $outDir

$dexItem = Get-Item $dexOut
Write-Host "Successfully built $($dexItem.FullName) ($($dexItem.Length) bytes)"
