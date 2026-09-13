# listwin.ps1 - list processes with main windows (all sessions view)
Get-Process | Where-Object { $_.MainWindowHandle -ne 0 } | ForEach-Object {
    "name={0} id={1} session={2} title='{3}'" -f $_.ProcessName, $_.Id, $_.SessionId, $_.MainWindowTitle
}
