@echo off
python "%~dp0..\build.py" route %*
exit /b %errorlevel%
