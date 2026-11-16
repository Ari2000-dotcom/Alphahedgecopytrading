@echo off
rem Starts the engine (paper mode) and the desktop app, sharing one token.
rem The token is made once and kept in %APPDATA%\AlphaHedge Desk\engine-token.txt.
rem It also seals the saved accounts, so do not delete that file.

set "TOKDIR=%APPDATA%\AlphaHedge Desk"
set "TOKFILE=%TOKDIR%\engine-token.txt"
if not exist "%TOKDIR%" mkdir "%TOKDIR%"
if not exist "%TOKFILE%" powershell -NoProfile -Command "-join ((48..57)+(97..122) | Get-Random -Count 32 | ForEach-Object {[char]$_}) | Set-Content -NoNewline '%TOKFILE%'"
set /p ENGINE_TOKEN=<"%TOKFILE%"

set "ACCOUNTS_FILE=%TOKDIR%\accounts.enc"

rem Engine in its own window, so its log stays visible.
start "kite-engine" /D "%~dp0kite-engine" "%~dp0kite-engine\target\release\kite-engine.exe"
timeout /t 2 /nobreak >nul

start "" "%~dp0alphahedge-desk\src-tauri\target\release\alphahedge-desk.exe"
