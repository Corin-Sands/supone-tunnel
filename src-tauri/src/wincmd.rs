//! Windows 子进程：隐藏控制台窗口。
//! GUI 壳（windows_subsystem）拉起 reg/powershell/tasklist/mihomo 等控制台程序时，
//! 不设 CREATE_NO_WINDOW 会狂闪黑窗，像病毒行为。Author: Corin-Sands

use std::process::Command;

/// 给 Windows 子进程加上 CREATE_NO_WINDOW；其它平台原样返回。
pub fn hide(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}
