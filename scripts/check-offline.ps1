# Fails if any networking or TLS crate is linked into the shipped binary.
# Build-time dependencies are excluded: skia-bindings downloads prebuilt Skia while compiling,
# but nothing network-capable ends up in labsetup.exe.
$ErrorActionPreference = 'Stop'
$banned = @('reqwest', 'hyper', 'ureq', 'curl', 'isahc', 'surf', 'attohttpc', 'h2', 'tungstenite',
            'async-tungstenite', 'rustls', 'native-tls', 'openssl', 'quinn', 'tokio-native-tls')

$tree = cargo tree -p lab-app -e normal --prefix none --format '{p}' | ForEach-Object { ($_ -split ' ')[0] } | Sort-Object -Unique
$found = $tree | Where-Object { $banned -contains $_ }
if ($found) {
    Write-Error "Network-capable crates in the runtime dependency tree: $($found -join ', ')"
    exit 1
}
Write-Output "Offline check passed: $($tree.Count) runtime crates, none network-capable."
