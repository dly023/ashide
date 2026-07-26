-- Finder 会把当前 icon layout 的窗口宽度收敛到最小 761px；这里直接声明持久化后的真实 bounds。
-- Ashide DMG 的 Finder 布局只在这里定义。构建与复验共用同一组断言，
-- 避免“写入一套、验证另一套”以及固定 delay 掩盖 Finder 尚未就绪。

on failIfPast(deadlineAt, messageText)
	if (current date) > deadlineAt then error messageText number 70
end failIfPast

on waitForDisk(volumeName, deadlineAt)
	tell application "Finder"
		repeat
			try
				if exists disk volumeName then return
			end try
			my failIfPast(deadlineAt, "Finder 未在截止时间前发现 DMG volume: " & volumeName)
			-- 仅在状态未满足时采样下一次状态，不是无条件固定等待。
			delay 0.1
		end repeat
	end tell
end waitForDisk

on waitForWindow(volumeName, deadlineAt)
	tell application "Finder"
		repeat
			try
				tell disk volumeName
					if exists container window then
						if visible of container window then return
					end if
				end tell
			end try
			my failIfPast(deadlineAt, "Finder 未在截止时间前显示 DMG window: " & volumeName)
			-- 仅在窗口尚未可见时采样下一次状态。
			delay 0.1
		end repeat
	end tell
end waitForWindow

on closeWindowIfOpen(volumeName, deadlineAt)
	tell application "Finder"
		try
			tell disk volumeName
				if exists container window then close container window
			end tell
		end try

		repeat
			try
				tell disk volumeName
					if not (exists container window) then return
					if not (visible of container window) then return
				end tell
			end try
			my failIfPast(deadlineAt, "Finder 未在截止时间前关闭旧 DMG window generation: " & volumeName)
			-- 仅在旧窗口 generation 仍可见时采样下一次状态。
			delay 0.1
		end repeat
	end tell
end closeWindowIfOpen

on formatBounds(windowBounds)
	return (item 1 of windowBounds as text) & "|" & (item 2 of windowBounds as text) & "|" & (item 3 of windowBounds as text) & "|" & (item 4 of windowBounds as text)
end formatBounds

on assertLayout(volumeName, appName, backgroundName, expectedWindowSize)
	tell application "Finder"
		tell disk volumeName
			set dmgWindow to container window
			if current view of dmgWindow is not icon view then error "DMG window 不是 icon view" number 71
			if toolbar visible of dmgWindow then error "DMG toolbar 必须隐藏" number 72
			if statusbar visible of dmgWindow then error "DMG status bar 必须隐藏" number 73
			set actualBounds to bounds of dmgWindow
			set actualWindowSize to {(item 3 of actualBounds) - (item 1 of actualBounds), (item 4 of actualBounds) - (item 2 of actualBounds)}
			if actualWindowSize is not expectedWindowSize then error "DMG window size 不匹配: bounds=" & my formatBounds(actualBounds) number 74

			set viewOptions to icon view options of dmgWindow
			if icon size of viewOptions is not 128 then error "DMG icon size 不匹配" number 75
			if arrangement of viewOptions is not not arranged then error "DMG icons 不应自动排列" number 76
			-- Finder 对 background picture 只可靠支持 setter；合法设置后 getter 仍会返回 -1728。
			-- 最终只读镜像由 shell 复验背景文件与 .icvp 的 backgroundType 持久化字段。
			if position of item appName is not {150, 250} then error "Ashide.app 位置不匹配" number 78
			if position of item "Applications" is not {550, 250} then error "Applications 位置不匹配" number 79
		end tell
	end tell
end assertLayout

on configureLayout(volumeName, mountPath, appName, backgroundName, deadlineAt)
	set expectedBounds to {10, 60, 771, 560}
	set expectedWindowSize to {761, 500}
	set dsStorePath to mountPath & "/.DS_Store"
	set initialHash to ""
	try
		set initialHash to do shell script "/bin/test -s " & quoted form of dsStorePath & " && /usr/bin/shasum -a 256 " & quoted form of dsStorePath
	end try

	my waitForDisk(volumeName, deadlineAt)
	tell application "Finder"
		tell disk volumeName
			open
		end tell
	end tell
	my waitForWindow(volumeName, deadlineAt)

	tell application "Finder"
		tell disk volumeName
			set dmgWindow to container window
			set current view of dmgWindow to icon view
			set toolbar visible of dmgWindow to false
			set statusbar visible of dmgWindow to false
			set bounds of dmgWindow to expectedBounds

			set viewOptions to icon view options of dmgWindow
			set icon size of viewOptions to 128
			set text size of viewOptions to 16
			set arrangement of viewOptions to not arranged
			set background picture of viewOptions to file (".background:" & backgroundName)
			set position of item appName to {150, 250}
			set position of item "Applications" to {550, 250}
			update without registering applications
		end tell
	end tell

	-- Finder 的可观察状态必须先完全等于发布合同，再允许进入落盘阶段。
	repeat
		try
			my assertLayout(volumeName, appName, backgroundName, expectedWindowSize)
			exit repeat
		on error errorMessage number errorNumber
			my failIfPast(deadlineAt, "Finder 未在截止时间前提交完整 DMG layout: " & errorMessage & " (" & errorNumber & ")")
			-- 仅在布局尚未提交时采样下一次状态。
			delay 0.1
		end try
	end repeat

	-- 保持最终窗口打开，等待 .DS_Store 相对配置前内容发生变化并连续稳定。
	-- 关闭窗口会让 Finder 重算 bounds，正是旧 create-dmg 需要固定 delay 和
	-- 尺寸抖动 workaround 的根因；现在由最终只读重挂验证持久化结果。
	set previousHash to ""
	set stableSamples to 0
	repeat
		try
			set currentHash to do shell script "/bin/test -s " & quoted form of dsStorePath & " && /usr/bin/shasum -a 256 " & quoted form of dsStorePath
			if currentHash is not initialHash then
				if currentHash is previousHash then
					set stableSamples to stableSamples + 1
				else
					set previousHash to currentHash
					set stableSamples to 0
				end if
			end if
			if stableSamples ≥ 3 then exit repeat
		on error
			set stableSamples to 0
		end try
		my failIfPast(deadlineAt, "Finder 未在截止时间前持久化稳定的最终 .DS_Store")
		-- 仅在 .DS_Store 尚未稳定时采样下一次内容哈希。
		delay 0.2
	end repeat
	do shell script "/bin/sync"
end configureLayout

on verifyPersistedLayout(volumeName, appName, backgroundName, deadlineAt)
	set expectedWindowSize to {761, 500}
	my waitForDisk(volumeName, deadlineAt)
	-- writable image 与最终 read-only image 具有同一 volume identity。Finder 可能复用
	-- 上一轮仍存活的 container window 内存状态，而不是从最终 .DS_Store 重新加载。
	-- 先建立明确的 closed-window generation boundary，再打开并验证持久化状态。
	my closeWindowIfOpen(volumeName, deadlineAt)

	tell application "Finder"
		tell disk volumeName
			open
		end tell
	end tell
	my waitForWindow(volumeName, deadlineAt)
	my assertLayout(volumeName, appName, backgroundName, expectedWindowSize)
	my closeWindowIfOpen(volumeName, deadlineAt)
end verifyPersistedLayout

on run argv
	if (count of argv) is not 5 then error "usage: configure_ashide_dmg.applescript <configure|verify> <volume> <mount> <app> <background>" number 64
	set operation to item 1 of argv
	set volumeName to item 2 of argv
	set mountPath to item 3 of argv
	set appName to item 4 of argv
	set backgroundName to item 5 of argv
	set deadlineAt to (current date) + 30

	if operation is "configure" then
		my configureLayout(volumeName, mountPath, appName, backgroundName, deadlineAt)
	else if operation is "verify" then
		my verifyPersistedLayout(volumeName, appName, backgroundName, deadlineAt)
	else
		error "未知 DMG layout operation: " & operation number 65
	end if
end run
