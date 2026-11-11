@echo off
set "ANADA_VAC_SCRIPT=%~dp0Configure-VAC.ps1"
powershell.exe -NoProfile -Command "& ([scriptblock]::Create([IO.File]::ReadAllText($env:ANADA_VAC_SCRIPT))) -Mode Restore"
set "RESULT=%ERRORLEVEL%"
if not "%RESULT%"=="0" echo VAC restore failed with exit code %RESULT%. Review the error above or in the administrator window.
if not "%ANADA_VAC_NO_PAUSE%"=="1" pause
exit /b %RESULT%
