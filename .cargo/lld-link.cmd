@echo off
setlocal

for /f "delims=" %%I in ('rustc --print sysroot') do set "RUST_SYSROOT=%%I"
if not defined RUST_SYSROOT (
    echo error: failed to locate the active Rust sysroot 1>&2
    exit /b 1
)

"%RUST_SYSROOT%\lib\rustlib\x86_64-pc-windows-msvc\bin\rust-lld.exe" %*
