Add-Type -AssemblyName PresentationFramework

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$runner = Join-Path $scriptRoot 'BackupRestore.ps1'

function New-TextBox([string]$text, [int]$width = 90) {
    $box = New-Object System.Windows.Controls.TextBox
    $box.Text = $text
    $box.Width = $width
    $box.Margin = '4'
    return $box
}

$window = New-Object System.Windows.Window
$window.Title = 'Windows BackupRestore'
$window.Width = 540
$window.Height = 430
$window.WindowStartupLocation = 'CenterScreen'
$panel = New-Object System.Windows.Controls.StackPanel
$panel.Margin = '16'
$window.Content = $panel

$title = New-Object System.Windows.Controls.TextBlock
$title.Text = 'Windows 系统备份 / 还原'
$title.FontSize = 22
$title.Margin = '4,4,4,12'
$panel.Children.Add($title) | Out-Null

$label = New-Object System.Windows.Controls.TextBlock
$label.Text = '操作模式'
$panel.Children.Add($label) | Out-Null
$mode = New-Object System.Windows.Controls.ComboBox
foreach ($item in @('backup', 'restore-existing', 'create-secondary')) { $mode.Items.Add($item) | Out-Null }
$mode.SelectedIndex = 0
$mode.Margin = '4'
$panel.Children.Add($mode) | Out-Null

function Add-DriveRow([string]$labelText, [string]$default) {
    $row = New-Object System.Windows.Controls.StackPanel
    $row.Orientation = 'Horizontal'
    $caption = New-Object System.Windows.Controls.TextBlock
    $caption.Text = $labelText
    $caption.Width = 150
    $caption.VerticalAlignment = 'Center'
    $box = New-TextBox $default 280
    $row.Children.Add($caption) | Out-Null
    $row.Children.Add($box) | Out-Null
    $panel.Children.Add($row) | Out-Null
    return $box
}

function Get-DriveIdentityText([string]$driveText) {
    $drive = $driveText.Trim().TrimEnd(':')
    if ($drive -notmatch '^[A-Za-z]$') { return "$driveText（盘符格式无效）" }
    try {
        $partition = Get-Partition -DriveLetter $drive -ErrorAction Stop
        $disk = Get-Disk -Number $partition.DiskNumber -ErrorAction Stop
        $volume = Get-Volume -DriveLetter $drive -ErrorAction Stop
        return ('{0}: 磁盘 GUID={1}; 分区 GUID={2}; 偏移={3}; 大小={4}; 文件系统={5}' -f
            $drive, $disk.UniqueId, $partition.Guid, $partition.Offset, $partition.Size, $volume.FileSystem)
    } catch {
        return "$driveText（无法读取分区身份：$($_.Exception.Message)）"
    }
}

$taskDrive = Add-DriveRow '任务卷盘符' 'D'
$sourceDrive = Add-DriveRow 'Windows 源盘符' 'C'
$imageDrive = Add-DriveRow '镜像卷盘符' 'D'
$targetDrive = Add-DriveRow '还原目标盘符' 'C'
$relativePath = Add-DriveRow '镜像相对路径' 'BackupRestore\Windows.wim'
$bootMenuName = Add-DriveRow '第二系统启动名称' 'Windows Backup'

$identity = New-Object System.Windows.Controls.TextBlock
$identity.Text = '点击开始时会重新读取磁盘 GUID、分区 GUID、偏移、大小和文件系统。'
$identity.TextWrapping = 'Wrap'
$identity.Margin = '4,8,4,4'
$panel.Children.Add($identity) | Out-Null

$warning = New-Object System.Windows.Controls.TextBlock
$warning.Text = '还原会覆盖目标分区；双系统只允许选择明确的 NTFS 目标。'
$warning.Foreground = [System.Windows.Media.Brushes]::DarkRed
$warning.TextWrapping = 'Wrap'
$warning.Margin = '4,12,4,4'
$panel.Children.Add($warning) | Out-Null

$button = New-Object System.Windows.Controls.Button
$button.Content = '开始'
$button.Width = 120
$button.HorizontalAlignment = 'Right'
$button.Margin = '4,12,4,4'
$panel.Children.Add($button) | Out-Null

$button.Add_Click({
    $selected = [string]$mode.SelectedItem
    $identityLines = @(
        (Get-DriveIdentityText $taskDrive.Text),
        (Get-DriveIdentityText $sourceDrive.Text),
        (Get-DriveIdentityText $imageDrive.Text),
        (Get-DriveIdentityText $targetDrive.Text)
    )
    $identity.Text = ($identityLines -join "`n")
    if ($identityLines | Where-Object { $_ -match '格式无效|无法读取' }) {
        [System.Windows.MessageBox]::Show('无法读取一个或多个分区身份，请修正盘符后重试。', '身份校验失败', 'OK', 'Error') | Out-Null
        return
    }
    $summary = "模式：$selected`n启动菜单名称：$($bootMenuName.Text)`n`n$($identityLines -join "`n")"
    if ($selected -in @('restore-existing', 'create-secondary')) {
        $answer = [System.Windows.MessageBox]::Show("将覆盖目标分区。`n`n$summary`n`n确认继续？", '二次确认', 'YesNo', 'Warning')
        if ($answer -ne 'Yes') { return }
    } else {
        $answer = [System.Windows.MessageBox]::Show("即将创建备份任务。`n`n$summary`n`n确认继续？", '确认任务', 'YesNo', 'Question')
        if ($answer -ne 'Yes') { return }
    }
    $arguments = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $runner,
        '-Operation', $selected, '-TaskDrive', $taskDrive.Text.Trim(':').Trim(),
        '-SourceDrive', $sourceDrive.Text.Trim(':').Trim(), '-ImageDrive', $imageDrive.Text.Trim(':').Trim(),
        '-TargetDrive', $targetDrive.Text.Trim(':').Trim(), '-ImageRelativePath', $relativePath.Text,
        '-BootMenuName', $bootMenuName.Text,
        '-AllowDestructive')
    try {
        Start-Process powershell.exe -Verb RunAs -Wait -ArgumentList $arguments
        [System.Windows.MessageBox]::Show('任务已提交。请查看任务目录中的 status.json 和 recovery.log。', 'BackupRestore') | Out-Null
    } catch {
        [System.Windows.MessageBox]::Show($_.Exception.Message, '提交失败', 'OK', 'Error') | Out-Null
    }
})

$window.ShowDialog() | Out-Null
