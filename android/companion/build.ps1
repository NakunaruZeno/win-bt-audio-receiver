$ErrorActionPreference = "Stop"

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$buildDir = Join-Path $scriptDir "build"
$apkOut = Join-Path $scriptDir "AetherSora-Companion.apk"

# Locate JDK tools (PATH, JAVA_HOME, or Android Studio JBR)
$javac = (Get-Command javac -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source -First 1)
if (-not $javac -and -not [string]::IsNullOrWhiteSpace($env:JAVA_HOME)) {
    $javaHomeJavac = Join-Path $env:JAVA_HOME "bin\javac.exe"
    if (Test-Path $javaHomeJavac) { $javac = $javaHomeJavac }
}
if (-not $javac) {
    $jbrJavac = "C:\Program Files\Android\Android Studio\jbr\bin\javac.exe"
    if (Test-Path $jbrJavac) { $javac = $jbrJavac }
}
if (-not $javac) { throw "javac.exe not found." }
$javaBinDir = Split-Path -Parent $javac
$keytool = Join-Path $javaBinDir "keytool.exe"
$env:PATH = "$javaBinDir;$env:PATH"

# Locate Android SDK tools
$sdkCandidates = @($env:ANDROID_HOME, $env:ANDROID_SDK_ROOT)
if (-not [string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
    $sdkCandidates += (Join-Path $env:LOCALAPPDATA "Android\Sdk")
}
$sdkRoot = $sdkCandidates |
    Where-Object { -not [string]::IsNullOrWhiteSpace($_) -and (Test-Path $_) } |
    Select-Object -First 1
if (-not $sdkRoot) { throw "Android SDK root not found." }

$androidJar = Get-ChildItem -Path (Join-Path $sdkRoot "platforms") -Recurse -Filter "android.jar" |
    Sort-Object -Property @{ Expression = {
        if ($_.Directory.Name -match '^android-(\d+)') { [int]$Matches[1] } else { 0 }
    }; Descending = $true }, @{ Expression = { $_.Directory.Name }; Descending = $true } |
    Select-Object -ExpandProperty FullName -First 1
if (-not $androidJar) { throw "android.jar not found under $sdkRoot\platforms" }

$btDir = Get-ChildItem -Path (Join-Path $sdkRoot "build-tools") -Directory |
    Sort-Object -Property @{ Expression = {
        if ($_.Name -match '^(\d+\.\d+\.\d+)') { [version]$Matches[1] } else { [version]"0.0.0" }
    }; Descending = $true }, @{ Expression = { $_.Name }; Descending = $true } |
    Select-Object -ExpandProperty FullName -First 1
if (-not $btDir) { throw "Android build-tools not found under $sdkRoot\build-tools" }

$aapt2 = Join-Path $btDir "aapt2.exe"
$d8Bat = Join-Path $btDir "d8.bat"
$zipalign = Join-Path $btDir "zipalign.exe"
$apksigner = Join-Path $btDir "apksigner.bat"

foreach ($tool in @($androidJar, $aapt2, $d8Bat, $zipalign, $apksigner)) {
    if (-not (Test-Path $tool)) { throw "Required Android SDK tool missing: $tool" }
}

if (Test-Path $buildDir) { Remove-Item -Recurse -Force $buildDir }
$resZip = Join-Path $buildDir "res.zip"
$genDir = Join-Path $buildDir "gen"
$objDir = Join-Path $buildDir "obj"
$dexDir = Join-Path $buildDir "dex"
New-Item -ItemType Directory -Path $genDir, $objDir, $dexDir | Out-Null

Write-Host "[1/6] Compiling Android resources with aapt2..."
& $aapt2 compile --dir (Join-Path $scriptDir "res") -o $resZip
if ($LASTEXITCODE -ne 0) { throw "aapt2 compile failed" }

Write-Host "[2/6] Linking resources & generating R.java..."
$unalignedApk = Join-Path $buildDir "unaligned.apk"
& $aapt2 link $resZip `
    -I $androidJar `
    --manifest (Join-Path $scriptDir "AndroidManifest.xml") `
    --java $genDir `
    -o $unalignedApk
if ($LASTEXITCODE -ne 0) { throw "aapt2 link failed" }

Write-Host "[3/6] Compiling Java sources..."
$javaFiles = @(Get-ChildItem -Path (Join-Path $scriptDir "src") -Recurse -Filter "*.java" | Select-Object -ExpandProperty FullName)
$javaFiles += @(Get-ChildItem -Path $genDir -Recurse -Filter "*.java" | Select-Object -ExpandProperty FullName)
& $javac --release 8 -cp $androidJar -d $objDir $javaFiles
if ($LASTEXITCODE -ne 0) { throw "javac failed" }

Write-Host "[4/6] Dexing bytecode with d8..."
$classFiles = Get-ChildItem -Path $objDir -Recurse -Filter "*.class" | Select-Object -ExpandProperty FullName
& $d8Bat --release --min-api 29 --lib $androidJar --output $dexDir $classFiles
if ($LASTEXITCODE -ne 0) { throw "d8 failed" }

Write-Host "[5/6] Adding classes.dex to APK & zipaligning..."
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip = [System.IO.Compression.ZipFile]::Open($unalignedApk, [System.IO.Compression.ZipArchiveMode]::Update)
[System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, (Join-Path $dexDir "classes.dex"), "classes.dex") | Out-Null
$zip.Dispose()

$alignedApk = Join-Path $buildDir "aligned.apk"
& $zipalign -f 4 $unalignedApk $alignedApk
if ($LASTEXITCODE -ne 0) { throw "zipalign failed" }

Write-Host "[6/6] Signing APK with local debug keystore..."
$keystore = Join-Path $scriptDir "debug.keystore"
if (-not (Test-Path $keystore)) {
    & $keytool -genkeypair -v -keystore $keystore -storepass android -alias aethersora -keypass android -keyalg RSA -keysize 2048 -validity 10000 -dname "CN=AetherSora,O=AetherSora,C=VN"
    if ($LASTEXITCODE -ne 0) { throw "keytool failed" }
}

& $apksigner sign --v4-signing-enabled false --ks $keystore --ks-pass pass:android --ks-key-alias aethersora --key-pass pass:android --out $apkOut $alignedApk
if ($LASTEXITCODE -ne 0) { throw "apksigner failed" }

Remove-Item -Recurse -Force $buildDir
$apkItem = Get-Item $apkOut
Write-Host "Successfully built $($apkItem.FullName) ($($apkItem.Length) bytes / $([math]::Round($apkItem.Length / 1KB, 2)) KB)"
