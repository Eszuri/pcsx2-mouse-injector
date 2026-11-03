# Script untuk menandatangani binary mouse-injector-pcsx2-gui.exe dengan Authenticode
# Mengurangi deteksi false-positive dan peringatan Windows Defender / SmartScreen

$certName = "PCSX2MouseInjector"
$cert = Get-ChildItem Cert:\CurrentUser\My -CodeSigningCert -ErrorAction SilentlyContinue | Where-Object { $_.Subject -like "*$certName*" } | Select-Object -First 1

if (-not $cert) {
    Write-Host "Membuat Code Signing Certificate lokal baru..." -ForegroundColor Cyan
    $cert = New-SelfSignedCertificate -Type CodeSigningCert -Subject "CN=$certName" -CertStoreLocation "Cert:\CurrentUser\My"
    
    # Memasukkan ke Trusted Root Certification Authorities akun pengguna
    $rootStore = [System.Security.Cryptography.X509Certificates.X509Store]::new("Root", "CurrentUser")
    $rootStore.Open("ReadWrite")
    $rootStore.Add($cert)
    $rootStore.Close()
    Write-Host "Sertifikat berhasil dipasang di Trusted Root!" -ForegroundColor Green
}

$exePath = Join-Path $PSScriptRoot "target\release\mouse-injector-pcsx2-gui.exe"
if (Test-Path $exePath) {
    Write-Host "Menandatangani executable: $exePath..." -ForegroundColor Cyan
    $sig = Set-AuthenticodeSignature -FilePath $exePath -Certificate $cert -TimestampServer "http://timestamp.digicert.com"
    if ($sig.Status -eq "Valid") {
        Write-Host "Binary BERHASIL ditandatangani secara valid!" -ForegroundColor Green
    } else {
        Write-Host "Status signature: $($sig.StatusMessage)" -ForegroundColor Yellow
    }
} else {
    Write-Host "File executable tidak ditemukan di $exePath. Jalankan 'cargo build --release' terlebih dahulu." -ForegroundColor Red
}
