$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# The caller supplies these environment values separately from this script.
# Explorer has already been launched; this helper only finds and raises its window.
$targetHandle = [IntPtr]::Zero
$foregroundHandle = [IntPtr]::Zero
$selectionConfirmed = $false
$failure = $null

try {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

public static class HerdrExplorerFocusNative {
    [DllImport("user32.dll")]
    public static extern bool ShowWindowAsync(IntPtr window, int command);
    [DllImport("user32.dll")]
    public static extern bool IsIconic(IntPtr window);
    [DllImport("user32.dll")]
    public static extern bool SetForegroundWindow(IntPtr window);
    [DllImport("user32.dll")]
    public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll", EntryPoint = "GetWindowLongW")]
    public static extern int GetWindowLong(IntPtr window, int index);
    [DllImport("user32.dll")]
    public static extern bool SetWindowPos(IntPtr window, IntPtr after,
        int x, int y, int width, int height, uint flags);
}
'@

    function Normalize-ExplorerPath([string] $path) {
        if ([string]::IsNullOrEmpty($path)) { return '' }
        return $path.Replace('/', '\').TrimEnd('\')
    }

    function Confirm-FileSelection($document, [string] $path) {
        $expected = Normalize-ExplorerPath $path
        $items = $document.SelectedItems()
        for ($index = 0; $index -lt $items.Count; $index++) {
            if ((Normalize-ExplorerPath $items.Item($index).Path) -ieq $expected) {
                return $true
            }
        }
        return $false
    }

    function Raise-ExplorerWindow([IntPtr] $handle, [bool] $allowTopmost) {
        if ([HerdrExplorerFocusNative]::IsIconic($handle)) {
            [void] [HerdrExplorerFocusNative]::ShowWindowAsync($handle, 9) # SW_RESTORE
        } else {
            [void] [HerdrExplorerFocusNative]::ShowWindowAsync($handle, 5) # SW_SHOW
        }
        [void] [HerdrExplorerFocusNative]::SetForegroundWindow($handle)
        if ([HerdrExplorerFocusNative]::GetForegroundWindow() -eq $handle) {
            return $true
        }

        # Try one temporary Z-order promotion. Preserve windows already topmost,
        # and always undo our promotion even when the foreground request fails.
        $alreadyTopmost = ([HerdrExplorerFocusNative]::GetWindowLong($handle, -20) -band 8) -ne 0
        if ($allowTopmost -and -not $alreadyTopmost) {
            try {
                [void] [HerdrExplorerFocusNative]::SetWindowPos(
                    $handle, [IntPtr]::new(-1), 0, 0, 0, 0, 0x53)
                [void] [HerdrExplorerFocusNative]::SetForegroundWindow($handle)
            } finally {
                [void] [HerdrExplorerFocusNative]::SetWindowPos(
                    $handle, [IntPtr]::new(-2), 0, 0, 0, 0, 0x53)
            }
        }
        return [HerdrExplorerFocusNative]::GetForegroundWindow() -eq $handle
    }

    if ([string]::IsNullOrWhiteSpace($env:HERDR_REVEAL_PATH)) {
        throw 'HERDR_REVEAL_PATH is empty'
    }
    if ($env:HERDR_REVEAL_IS_DIR -notin @('0', '1', 'false', 'true')) {
        throw 'HERDR_REVEAL_IS_DIR must be 0, 1, false, or true'
    }
    $isDirectory = $env:HERDR_REVEAL_IS_DIR -in @('1', 'true')
    $targetPath = [IO.Path]::GetFullPath($env:HERDR_REVEAL_PATH)
    $directory = if ($isDirectory) { $targetPath } else { [IO.Path]::GetDirectoryName($targetPath) }
    $expectedDirectory = Normalize-ExplorerPath $directory
    $fileName = [IO.Path]::GetFileName($targetPath)
    $shell = New-Object -ComObject Shell.Application
    $raisedWindows = New-Object 'System.Collections.Generic.HashSet[long]'
    $timer = [Diagnostics.Stopwatch]::StartNew()

    do {
        # Enumerate again each time: Explorer may still be creating its window.
        $windows = $shell.Windows()
        for ($index = $windows.Count - 1; $index -ge 0; $index--) {
            try {
                $window = $windows.Item($index)
                $document = $window.Document
                $folder = $document.Folder
                if ((Normalize-ExplorerPath $folder.Self.Path) -ine $expectedDirectory) {
                    continue
                }
                $targetHandle = [IntPtr] $window.HWND
                if ($targetHandle -eq [IntPtr]::Zero) { continue }
                $selectionConfirmed = $isDirectory
                if (-not $isDirectory) {
                    $selectionConfirmed = Confirm-FileSelection $document $targetPath
                    if (-not $selectionConfirmed) {
                        $item = $folder.ParseName($fileName)
                        if ($null -ne $item) {
                            # SELECT | DESELECTOTHERS | ENSUREVISIBLE | FOCUSED
                            [void] $document.SelectItem($item, 29)
                            $selectionConfirmed = Confirm-FileSelection $document $targetPath
                        }
                    }
                }
                if (-not $selectionConfirmed) { continue }
                $allowTopmost = $raisedWindows.Add($targetHandle.ToInt64())
                if (Raise-ExplorerWindow $targetHandle $allowTopmost) { break }
            } catch {
                # A window can close or finish navigating during COM enumeration.
                continue
            }
        }
        $foregroundHandle = [HerdrExplorerFocusNative]::GetForegroundWindow()
        if ($targetHandle -ne [IntPtr]::Zero -and $selectionConfirmed -and
            $foregroundHandle -eq $targetHandle) {
            break
        }
        Start-Sleep -Milliseconds 100
    } while ($timer.ElapsedMilliseconds -lt 3000)

    if ($targetHandle -eq [IntPtr]::Zero) {
        $failure = 'Explorer did not expose the target directory within 3 seconds'
    } elseif (-not $selectionConfirmed) {
        $failure = 'Explorer did not confirm the requested file selection'
    } elseif ($foregroundHandle -ne $targetHandle) {
        $failure = 'Windows did not grant foreground focus to the Explorer window'
    }
} catch {
    $failure = $_.Exception.Message
}

$success = $null -eq $failure -and $targetHandle -ne [IntPtr]::Zero -and
    $selectionConfirmed -and $foregroundHandle -eq $targetHandle
[pscustomobject] @{
    success = $success
    foreground = $foregroundHandle.ToInt64()
    target = $targetHandle.ToInt64()
    selection_confirmed = $selectionConfirmed
    error = $failure
} | ConvertTo-Json -Compress
if (-not $success) { exit 1 }
