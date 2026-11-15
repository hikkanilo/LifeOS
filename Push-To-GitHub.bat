@echo off
chcp 65001 >nul
setlocal
cd /d "%~dp0"

echo ========================================================
echo   LifeOS - Отправка репозитория на GitHub (git push)
echo ========================================================
echo.

git push origin main
if %ERRORLEVEL% EQU 0 (
    echo.
    echo ========================================================
    echo   [УСПЕХ] Все файлы успешно загружены на GitHub!
    echo ========================================================
) else (
    echo.
    echo [!] Не удалось отправить. Проверьте авторизацию в GitHub
    echo     или используйте GitHub Desktop: File - Add Local Repository.
)

echo.
pause
