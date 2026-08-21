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
$homeInfo.Text = @(
    '本工具只负责创建并验证任务，真正的 DISM、格式化和 BCDBoot 操作在 WinRE 中执行。'
    ''
    '推荐顺序：先运行“探测”确认 UEFI/GPT/WinRE/BitLocker 与磁盘身份，再创建备份；还原前必须核对目标盘的 GUID、偏移、容量和文件系统。'
    ''
    '状态含义：已准备表示载荷已写入 WinRE 但尚未证明恢复成功；运行中/失败/成功以任务目录的 status.json、Recovery.log 和 prepare.log 为准。'
    ''
    '当前主机无法替代真实 Windows/WinRE 验收；不要把本页的静态信息当成重启后成功证据。'
) -join "`n"
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
        $secureBoot = 'unknown'
        try {
            $secureBoot = if (Confirm-SecureBootUEFI -ErrorAction Stop) { 'on' } else { 'off' }
        } catch { }
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
            "Secure Boot：$secureBoot"
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
# 探测是无破坏诊断入口，作为默认模式，避免窗口刚打开就准备备份或还原任务。
$mode.SelectedIndex = 0
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
        return ('{0}: 磁盘 GUID={1}; 分区 GUID={2}; 卷 GUID={3}; 类型={4}; 偏移={5}; 大小={6}; 剩余={7}; 文件系统={8}; 序列号={9}' -f
            $drive, $disk.UniqueId, $partition.Guid, $volume.UniqueId, $partition.GptType, $partition.Offset, $partition.Size, $volume.SizeRemaining, $volume.FileSystem, $volume.SerialNumber)
    } catch {
        return "$driveText（无法读取分区身份：$($_.Exception.Message)）"
    }
}

$systemDrive = if ($env:SystemDrive -match '^[A-Za-z]:$') { $env:SystemDrive.TrimEnd(':') } else { 'C' }
$candidateLetters = @(Get-Volume -ErrorAction SilentlyContinue | Where-Object {
    $_.DriveLetter -and $_.FileSystem -eq 'NTFS'
} | ForEach-Object { "$($_.DriveLetter)".ToUpperInvariant() } | Sort-Object -Unique)
$taskDefault = $candidateLetters | Where-Object { $_ -ne $systemDrive.ToUpperInvariant() } | Select-Object -First 1
$imageDefault = $candidateLetters | Where-Object {
    $_ -ne $systemDrive.ToUpperInvariant() -and $_ -ne "$taskDefault".ToUpperInvariant()
} | Select-Object -First 1
$taskDrive = Add-DriveRow '任务卷盘符' ($(if ($taskDefault) { [string]$taskDefault } else { $systemDrive }))
$sourceDrive = Add-DriveRow 'Windows 源盘符' $systemDrive
$imageDrive = Add-DriveRow '镜像卷盘符' ([string]$imageDefault)
$targetDrive = Add-DriveRow '还原目标盘符' $systemDrive
$relativePath = Add-DriveRow '镜像相对路径' 'BackupRestore\Windows.wim'
$wimIndex = Add-DriveRow 'WIM 索引' '1'
$bootMenuName = Add-DriveRow '第二系统启动名称' 'Windows Backup'
$imageInfo = New-Object System.Windows.Controls.TextBlock
$imageInfo.Text = '镜像信息：点击“读取镜像信息”检查 WIM 文件和 metadata.json。'
$imageInfo.TextWrapping = 'Wrap'
$imageInfo.Margin = '4,8,4,4'
$panel.Children.Add($imageInfo) | Out-Null
$readImage = New-Object System.Windows.Controls.Button
$readImage.Content = '读取镜像信息'
$readImage.Width = 140
$readImage.Margin = '4,4,4,4'
$panel.Children.Add($readImage) | Out-Null

function Read-ImageInfo {
    try {
        $drive = $imageDrive.Text.Trim().TrimEnd(':')
        if ($drive -notmatch '^[A-Za-z]$') { throw '镜像卷盘符格式无效。' }
        $candidate = Join-Path "$drive`:\" $relativePath.Text
        if (-not (Test-Path -LiteralPath $candidate)) { throw "WIM 不存在：$candidate" }
        $file = Get-Item -LiteralPath $candidate -ErrorAction Stop
        $actualHash = (Get-FileHash -LiteralPath $candidate -Algorithm SHA256).Hash.ToLowerInvariant()
        $metadataPath = Join-Path (Split-Path -Parent $candidate) 'metadata.json'
        if (-not (Test-Path -LiteralPath $metadataPath)) { throw "metadata.json 不存在：$metadataPath" }
        $metadata = Get-Content -LiteralPath $metadataPath -Raw | ConvertFrom-Json
        $hashState = if ($metadata.imageSha256 -and $actualHash -eq "$($metadata.imageSha256)".ToLowerInvariant()) { '匹配' } else { '不匹配' }
        return "镜像：$candidate`n大小：$($file.Length) bytes`nSHA-256（实际）：$actualHash`nSHA-256（metadata）：$($metadata.imageSha256) [$hashState]`nWindows：$($metadata.windowsEdition) build=$($metadata.windowsBuild) arch=$($metadata.architecture)`n源分区大小：$($metadata.source.partitionSize)；最小目标：$($metadata.minimumTargetSize)"
    } catch {
        return "镜像信息读取失败：$($_.Exception.Message)"
    }
}
$readImage.Add_Click({ $imageInfo.Text = Read-ImageInfo })

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
$paths.Text = '任务信息和日志将在准备脚本返回后显示；真实终态以任务目录中的 status.json 为准。'
$paths.TextWrapping = 'Wrap'
$paths.Margin = '4,12,4,4'
$resultPanel.Children.Add($paths) | Out-Null
$refreshTask = New-Object System.Windows.Controls.Button
$refreshTask.Content = '刷新任务状态'
$refreshTask.Width = 140
$refreshTask.Margin = '4,8,4,4'
$resultPanel.Children.Add($refreshTask) | Out-Null

$lastTaskPath = Join-Path $env:ProgramData 'BackupRestore\last-task.json'
function Read-LastTaskStatus {
    if (-not (Test-Path $lastTaskPath)) {
        return '尚未找到最近任务记录。'
    }
    try {
        $record = Get-Content -LiteralPath $lastTaskPath -Raw | ConvertFrom-Json
        $statusValue = 'status.json 不可读'
        if (Test-Path $record.statusJson) {
            $statusValue = (Get-Content -LiteralPath $record.statusJson -Raw | ConvertFrom-Json | ConvertTo-Json -Compress)
        }
        $imageSummary = "镜像：$($record.imagePath)"
        if (Test-Path $record.metadataPath) {
            $metadata = Get-Content -LiteralPath $record.metadataPath -Raw | ConvertFrom-Json
            $imageSummary = "镜像：$($record.imagePath)`n镜像大小：$($metadata.imageSize) bytes；创建时间：$($metadata.created)；SHA-256：$($metadata.imageSha256)"
        }
        $paths.Text = "任务 ID：$($record.taskId)`n任务目录：$($record.taskRoot)`n准备日志：$($record.prepareLog)`n恢复日志：$($record.recoveryLog)`n$imageSummary"
        return "最近任务：$($record.operation)`n$statusValue`n$imageSummary`n`n注意：准备成功不等于 WinRE 恢复成功。"
    } catch {
        return "读取任务状态失败：$($_.Exception.Message)"
    }
}
$refreshTask.Add_Click({ $status.Text = Read-LastTaskStatus })

function Update-ModeFields {
    $selected = [string]$mode.SelectedItem
    $isSecondary = $selected -eq 'create-secondary'
    $bootMenuName.IsEnabled = $isSecondary
    $targetDrive.IsEnabled = $isSecondary -or $selected -eq 'restore-existing'
    $relativePath.IsEnabled = $selected -ne 'probe'
    $wimIndex.IsEnabled = $selected -ne 'probe'
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
    $parsedWimIndex = 0
    if (-not [int]::TryParse($wimIndex.Text.Trim(), [ref]$parsedWimIndex) -or $parsedWimIndex -lt 1) {
        $status.Text = 'WIM 索引必须是大于等于 1 的整数。'
        [System.Windows.MessageBox]::Show($status.Text, '参数校验失败', 'OK', 'Error') | Out-Null
        return
    }
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
        '-WimIndex', [string]$parsedWimIndex, '-BootMenuName', $bootMenuName.Text)
    # 只有两种还原模式允许破坏性参数；probe/backup 永远不带它。
    if ($selected -in @('restore-existing', 'create-secondary')) { $arguments += '-AllowDestructive' }
    try {
        $argumentLine = ($arguments | ForEach-Object { Quote-ProcessArgument ([string]$_) }) -join ' '
        $process = Start-Process powershell.exe -Verb RunAs -Wait -PassThru -ArgumentList $argumentLine
        if ($process.ExitCode -eq 0) {
            $status.Text = Read-LastTaskStatus
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
