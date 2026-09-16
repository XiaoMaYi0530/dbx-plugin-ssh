@echo off
rem Windows wrapper: run scripts/build.sh through Git Bash from cmd/PowerShell
rem or by double-clicking. All arguments are passed through.
setlocal
set "BASH=C:\Program Files\Git\bin\bash.exe"
if not exist "%BASH%" (
  echo Git Bash not found at "%BASH%" >&2
  exit /b 1
)

rem MSVC toolchain: cargo must link with MSVC link.exe, but Git Bash puts
rem its own /usr/bin/link.exe (coreutils) first in PATH. Load vcvars64 (for
rem INCLUDE/LIB) and pin the linker by absolute path so PATH order cannot
rem shadow it.
if not defined VCToolsInstallDir (
  if exist "C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Auxiliary\Build\vcvars64.bat" (
    call "C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
  )
)
if defined VCToolsInstallDir set "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER=%VCToolsInstallDir%bin\Hostx64\x64\link.exe"

rem openssl-src: use Strawberry Perl; Git Bash's MSYS perl lacks
rem Locale::Maketext::Simple and OpenSSL's Configure fails with it.
if not defined OPENSSL_SRC_PERL if exist "C:\Strawberry\perl\bin\perl.exe" set "OPENSSL_SRC_PERL=C:\Strawberry\perl\bin\perl.exe"

pushd %~dp0..
"%BASH%" --login scripts/build.sh %*
set "RC=%ERRORLEVEL%"
popd
exit /b %RC%
