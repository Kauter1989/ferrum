@echo off
rem Build the FERRUM WebGPU module the panel loads.
rem
rem   integrations\dsh\panel\build.cmd
rem
rem Needs the wasm32 target and the wasm-bindgen CLI of the same version as the
rem wasm-bindgen crate the workspace resolves:
rem
rem   rustup target add wasm32-unknown-unknown
rem   cargo install wasm-bindgen-cli --version 0.2.129 --locked
rem
rem The output (ferrum_wasm.js, ferrum_wasm_bg.wasm) is a build artifact and is
rem not committed: rebuild it on the machine that runs the panel.
setlocal
set HERE=%~dp0
set ROOT=%HERE%..\..\..
pushd "%ROOT%"

cargo build --release -p ferrum-wasm --target wasm32-unknown-unknown || goto :error
wasm-bindgen --target web --out-dir "%HERE%." --out-name ferrum_wasm --no-typescript ^
  target\wasm32-unknown-unknown\release\ferrum_wasm.wasm || goto :error

echo.
echo Built %HERE%ferrum_wasm.js and %HERE%ferrum_wasm_bg.wasm
echo Serve the folder over http and open stand.html:
echo   python -m http.server 45999 --directory "%HERE%."
popd
exit /b 0

:error
echo Build failed.
popd
exit /b 1