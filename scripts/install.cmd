@echo off
rem Windows wrapper: run scripts/install.sh through Git Bash from cmd/PowerShell
rem or by double-clicking. All arguments are passed through, e.g.
rem   scripts\install.cmd --reinstall
rem Requires DBX_HOST_WORKTREE to point at the DBX host checkout.
setlocal
set "BASH=C:\Program Files\Git\bin\bash.exe"
if not exist "%BASH%" (
  echo Git Bash not found at "%BASH%" >&2
  exit /b 1
)
if not defined DBX_HOST_WORKTREE (
  echo DBX_HOST_WORKTREE is not set; point it at the DBX host checkout first, e.g. >&2
  echo   set DBX_HOST_WORKTREE=D:\path\to\dbx >&2
  exit /b 1
)

rem See build.cmd: pin MSVC link.exe and Strawberry Perl for the host
rem worktree's cargo builds (install_plugin example pulls openssl-sys).
if not defined VCToolsInstallDir (
  if exist "C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Auxiliary\Build\vcvars64.bat" (
    call "C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
  )
)
if defined VCToolsInstallDir set "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER=%VCToolsInstallDir%bin\Hostx64\x64\link.exe"
if not defined OPENSSL_SRC_PERL if exist "C:\Strawberry\perl\bin\perl.exe" set "OPENSSL_SRC_PERL=C:\Strawberry\perl\bin\perl.exe"

pushd %~dp0..
"%BASH%" --login scripts/install.sh %*
set "RC=%ERRORLEVEL%"
popd
exit /b %RC%
