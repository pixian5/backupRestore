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

function Quote-ProcessArgument([string]$value) {
    if ($null -eq $value) { return '""' }
    if ($value -notmatch '[\s"]') { return $value }
    $escaped = [regex]::Replace($value, '(\\*)"', '$1$1\"')
    $escaped = [regex]::Replace($escaped, '(\\+)$', '$1$1')
    return '"' + $escaped + '"'
}

function New-Tab([string]$header) {
    $tab = New-Object System.Windows.Controls.TabItem
    $tab.Header = $header
    $content = New-Object System.Windows.Controls.StackPanel
    $content.Margin = '14'
    $tab.Content = $content
    $tabs.Items.Add($tab) | Out-Null
    return $content
}

$window = New-Object System.Windows.Window
$window.Title = 'Windows BackupRestore'
$window.Width = 700
$window.Height = 700
$window.WindowStartupLocation = 'CenterScreen'

$scroll = New-Object System.Windows.Controls.ScrollViewer
$scroll.VerticalScrollBarVisibility = 'Auto'
$rootPanel = New-Object System.Windows.Controls.StackPanel
$rootPanel.Margin = '16'
$scroll.Content = $rootPanel
$window.Content = $scroll

$title = New-Object System.Windows.Controls.TextBlock
$title.Text = 'Windows 系统备份 / 还原'
$title.FontSize = 22
$title.Margin = '4,4,4,12'
$rootPanel.Children.Add($title) | Out-Null

$tabs = New-Object System.Windows.Controls.TabControl
$rootPanel.Children.Add($tabs) | Out-Null
$homePanel = New-Tab '首页 / 环境'
$operationPanel = New-Tab '备份与还原'
$resultPanel = New-Tab '任务结果 / 日志'

$homeTitle = New-Object System.Windows.Controls.TextBlock
$homeTitle.Text = '开始前检查'
$homeTitle.FontSize = 18
$homeTitle.Margin = '4,4,4,8'
$homePanel.Children.Add($homeTitle) | Out-Null
$homeInfo = New-Object System.Windows.Controls.TextBlock
$homeInfo.Text = @'
本工具只负责创建并验证任务，真正的 DISM、格式化和 BCDBoot 操作在 WinRE 中执行。

推荐顺序：先运行“探测”确认 UEFI/GPT/WinRE/BitLocker 与磁盘身份，再创建备份；还原前必须核对目标盘的 GUID、偏移、容量和文件系统。

状态含义：已准备表示载荷已写入 WinRE 但尚未证明恢复成功；运行中/失败/成功以任务目录的 status.json、Recovery.log 和 prepare.log 为准。

当前主机无法替代真实 Windows/WinRE 验收；不要把本页的静态信息当成重启后成功证据。
'@
$homeInfo.TextWrapping = 'Wrap'
$homePanel.Children.Add($homeInfo) | Out-Null

$environment = New-Object System.Windows.Controls.TextBox
$environment.IsReadOnly = $true
$environment.TextWrapping = 'Wrap'
$environment.Height = 150
$environment.Margin = '4,14,4,4'
$environment.Text = '点击“刷新环境”读取当前 Windows、UEFI、磁盘和 WinRE 状态。'
$homePanel.Children.Add($environment) | Out-Null
$refresh = New-Object System.Windows.Controls.Button
$refresh.Content = '刷新环境信息'
$refresh.Width = 150
$refresh.Margin = '4,8,4,4'
$homePanel.Children.Add($refresh) | Out-Null

function Get-EnvironmentText {
    try {
        $os = Get-CimInstance Win32_OperatingSystem -ErrorAction Stop
        $firmware = (Get-ComputerInfo -Property BiosFirmwareType -ErrorAction Stop).BiosFirmwareType
        $bootDisk = Get-Disk -ErrorAction Stop | Where-Object IsBoot -eq $true | Select-Object -First 1
        $reagent = reagentc.exe /info 2>&1 | Out-String
        $bitlocker = if (Get-Command Get-BitLockerVolume -ErrorAction SilentlyContinue) {
            $state = Get-BitLockerVolume -MountPoint 'C:' -ErrorAction SilentlyContinue
            if ($state) { "$($state.ProtectionStatus)" } else { 'unknown' }
        } else { 'cmdlet unavailable' }
        $winreState = if ($reagent -match '(?i)(GLOBALROOT|Recovery\\WindowsRE)') { 'registered' } else { 'not detected' }
        $volumes = @(Get-Volume -ErrorAction Stop | Where-Object {
            $_.DriveLetter -and $_.FileSystem -eq 'NTFS'
        } | ForEach-Object {
            $drive = [string]$_.DriveLetter
            $root = "$drive`:\"
            $partition = Get-Partition -DriveLetter $drive -ErrorAction SilentlyContinue
            $disk = if ($partition) { Get-Disk -Number $partition.DiskNumber -ErrorAction SilentlyContinue } else { $null }
            $kind = if (Test-Path (Join-Path $root 'Windows\System32\config\SYSTEM')) { 'Windows 安装' } else { 'NTFS 候选目标' }
            "  $drive`: [$kind] disk=$($disk.UniqueId) partition=$($partition.Guid) offset=$($partition.Offset) size=$($partition.Size) free=$($_.SizeRemaining)"
        })
        return @(
            "Windows：$($os.Caption) build=$($os.BuildNumber) architecture=$env:PROCESSOR_ARCHITECTURE"
            "Firmware：$firmware; boot disk=$($bootDisk.Number) partitionStyle=$($bootDisk.PartitionStyle)"
            "BitLocker(C:)：$bitlocker"
            "WinRE：$winreState"
            '可识别的 NTFS 卷：'
            $volumes
            "检查时间：$((Get-Date).ToString('o'))"
        ) -join "`n"
    } catch {
        return "环境检查失败：$($_.Exception.Message)"
    }
}
$refresh.Add_Click({ $environment.Text = Get-EnvironmentText })

$panel = $operationPanel
$label = New-Object System.Windows.Controls.TextBlock
$label.Text = '操作模式'
$panel.Children.Add($label) | Out-Null
$mode = New-Object System.Windows.Controls.ComboBox
foreach ($item in @('probe', 'backup', 'restore-existing', 'create-secondary')) { $mode.Items.Add($item) | Out-Null }
# 探测是诊断入口，但不应成为普通用户误点“开始”时的默认任务。
$mode.SelectedIndex = 1
$mode.Margin = '4'
$panel.Children.Add($mode) | Out-Null

function Add-DriveRow([string]$labelText, [string]$default) {
    $row = New-Object System.Windows.Controls.StackPanel
    $row.Orientation = 'Horizontal'
    $caption = New-Object System.Windows.Controls.TextBlock
    $caption.Text = $labelText
    $caption.Width = 170
    $caption.VerticalAlignment = 'Center'
    $box = New-TextBox $default 400
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
        return ('{0}: 磁盘 GUID={1}; 分区 GUID={2}; 偏移={3}; 大小={4}; 剩余={5}; 文件系统={6}' -f
            $drive, $disk.UniqueId, $partition.Guid, $partition.Offset, $partition.Size, $volume.SizeRemaining, $volume.FileSystem)
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
$identity.Text = '点击创建任务时会重新读取磁盘 GUID、分区 GUID、偏移、大小、剩余空间和文件系统。'
$identity.TextWrapping = 'Wrap'
$identity.Margin = '4,8,4,4'
$panel.Children.Add($identity) | Out-Null

$warning = New-Object System.Windows.Controls.TextBlock
$warning.Foreground = [System.Windows.Media.Brushes]::DarkRed
$warning.TextWrapping = 'Wrap'
$warning.Margin = '4,12,4,4'
$panel.Children.Add($warning) | Out-Null

$button = New-Object System.Windows.Controls.Button
$button.Content = '创建任务'
$button.Width = 140
$button.HorizontalAlignment = 'Right'
$button.Margin = '4,12,4,4'
$panel.Children.Add($button) | Out-Null

$resultTitle = New-Object System.Windows.Controls.TextBlock
$resultTitle.Text = '任务执行结果（只显示准备脚本结果，不代替 WinRE 验收）'
$resultTitle.FontSize = 16
$resultTitle.Margin = '4,4,4,8'
$resultPanel.Children.Add($resultTitle) | Out-Null
$status = New-TextBox '尚未提交任务。' 620
$status.IsReadOnly = $true
$status.TextWrapping = 'Wrap'
$status.Height = 120
$resultPanel.Children.Add($status) | Out-Null
$paths = New-Object System.Windows.Controls.TextBlock
$paths.Text = '日志位置：C:\ProgramData\BackupRestore\logs\prepare.log；任务目录由准备脚本输出。'
$paths.TextWrapping = 'Wrap'
$paths.Margin = '4,12,4,4'
$resultPanel.Children.Add($paths) | Out-Null

function Update-ModeFields {
    $selected = [string]$mode.SelectedItem
    $isSecondary = $selected -eq 'create-secondary'
    $bootMenuName.IsEnabled = $isSecondary
    $targetDrive.IsEnabled = $isSecondary -or $selected -eq 'restore-existing'
    if ($selected -eq 'restore-existing') {
        $targetDrive.Text = $sourceDrive.Text
        $targetDrive.IsEnabled = $false
    }
    if ($selected -eq 'probe') {
        $targetDrive.IsEnabled = $false
        $imageDrive.IsEnabled = $false
    } else {
        $imageDrive.IsEnabled = $true
    }
    switch ($selected) {
        'probe' { $warning.Text = '无破坏探测：验证 WinRE 载荷和卷身份，不会格式化、应用镜像或重启。' }
        'backup' { $warning.Text = '备份会在 WinRE 中捕获源 Windows 分区；不会格式化任何分区。' }
        'restore-existing' { $warning.Text = '单系统还原会格式化并覆盖当前 Windows 源分区；必须二次确认。' }
        'create-secondary' { $warning.Text = '双系统模式会格式化明确选择的 NTFS 目标分区，并保留现有启动项。' }
    }
}
$mode.Add_SelectionChanged({ Update-ModeFields })
$sourceDrive.Add_TextChanged({
    if ([string]$mode.SelectedItem -eq 'restore-existing') { $targetDrive.Text = $sourceDrive.Text }
})
Update-ModeFields

$button.Add_Click({
    $selected = [string]$mode.SelectedItem
    $identityLines = @(
        (Get-DriveIdentityText $taskDrive.Text),
        (Get-DriveIdentityText $sourceDrive.Text),
        (Get-DriveIdentityText $imageDrive.Text),
        (Get-DriveIdentityText $targetDrive.Text)
    )
    $identity.Text = ($identityLines -join "`n")
    $tabs.SelectedIndex = 2
    $status.Text = '正在执行管理员准备脚本，请等待返回…'
    if ($identityLines | Where-Object { $_ -match '格式无效|无法读取' }) {
        $status.Text = '身份校验失败：请修正盘符后重试。'
        [System.Windows.MessageBox]::Show($status.Text, '身份校验失败', 'OK', 'Error') | Out-Null
        return
    }
    $summary = "模式：$selected`n启动菜单名称：$($bootMenuName.Text)`n`n$($identityLines -join "`n")"
    if ($selected -in @('restore-existing', 'create-secondary')) {
        $answer = [System.Windows.MessageBox]::Show("将覆盖目标分区。`n`n$summary`n`n确认继续？", '二次确认', 'YesNo', 'Warning')
        if ($answer -ne 'Yes') { $status.Text = '用户取消了破坏性任务。'; return }
    } else {
        $answer = [System.Windows.MessageBox]::Show("即将创建任务。`n`n$summary`n`n确认继续？", '确认任务', 'YesNo', 'Question')
        if ($answer -ne 'Yes') { $status.Text = '用户取消了任务。'; return }
    }
    $arguments = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $runner,
        '-Operation', $selected, '-TaskDrive', $taskDrive.Text.Trim(':').Trim(),
        '-SourceDrive', $sourceDrive.Text.Trim(':').Trim(), '-ImageDrive', $imageDrive.Text.Trim(':').Trim(),
        '-TargetDrive', $targetDrive.Text.Trim(':').Trim(), '-ImageRelativePath', $relativePath.Text,
        '-BootMenuName', $bootMenuName.Text)
    # 只有两种还原模式允许破坏性参数；probe/backup 永远不带它。
    if ($selected -in @('restore-existing', 'create-secondary')) { $arguments += '-AllowDestructive' }
    try {
        $argumentLine = ($arguments | ForEach-Object { Quote-ProcessArgument ([string]$_) }) -join ' '
        $process = Start-Process powershell.exe -Verb RunAs -Wait -PassThru -ArgumentList $argumentLine
        if ($process.ExitCode -eq 0) {
            $status.Text = "任务已准备（不是恢复成功）。`n请查看任务目录中的 status.json、Recovery.log 和 prepare.log；真实完成状态须在 WinRE 重启后确认。"
            [System.Windows.MessageBox]::Show($status.Text, 'BackupRestore') | Out-Null
        } else {
            $status.Text = "任务准备失败，退出码：$($process.ExitCode)。"
            [System.Windows.MessageBox]::Show("任务准备失败，退出码：$($process.ExitCode)。请查看 C:\ProgramData\BackupRestore\logs\prepare.log。", '提交失败', 'OK', 'Error') | Out-Null
        }
    } catch {
        $status.Text = "提交失败：$($_.Exception.Message)"
        [System.Windows.MessageBox]::Show($status.Text, '提交失败', 'OK', 'Error') | Out-Null
    }
})

$window.ShowDialog() | Out-Null
