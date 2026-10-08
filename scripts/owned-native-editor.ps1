param(
    [Parameter(Mandatory=$true)][string]$FixtureId,
    [Parameter(Mandatory=$true)][ValidateSet('edit','rich','readonly','password')][string]$Kind,
    [Parameter(Mandatory=$true)][string]$ScratchDir,
    [ValidateRange(1,2147483647)][int]$ProbeProcessId
)
# Dedicated native fixture only. No microphone, cloud service, credentials,
# user files or other applications are accessed. The parent verifies our PID.
$ErrorActionPreference = 'Stop'
if ($FixtureId -notmatch '^[0-9a-f]{32}$' -or -not [IO.Path]::IsPathRooted($ScratchDir)) {
    throw 'A UUID fixture ID and explicit absolute scratch directory are required'
}
$speakScratch = [IO.Path]::GetFullPath($ScratchDir)
New-Item -ItemType Directory -Path $speakScratch -Force | Out-Null
$env:TEMP = $speakScratch
$env:TMP = $speakScratch
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type -TypeDefinition @'
using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Windows.Forms;
using System.Drawing;
public sealed class SpeakNativeFixture : Form {
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern IntPtr CreateWindowExW(uint ex, string cls, string title, uint style, int x, int y, int width, int height, IntPtr parent, IntPtr menu, IntPtr module, IntPtr param);
    [DllImport("user32.dll")] static extern IntPtr SetFocus(IntPtr hwnd);
    [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr hwnd);
    [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] static extern bool AllowSetForegroundWindow(uint processId);
    [DllImport("user32.dll")] static extern IntPtr SendMessageW(IntPtr hwnd, uint msg, IntPtr wparam, IntPtr lparam);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] static extern IntPtr LoadLibraryW(string path);
    private readonly string kind, fixture, directory;
    private readonly uint probeProcessId;
    private IntPtr editor, second;
    private Timer shutdownTimer;
    private bool closeRequested;
    public SpeakNativeFixture(string kind, string fixture, string directory, uint probeProcessId) {
        this.kind = kind; this.fixture = fixture; this.directory = directory;
        this.probeProcessId = probeProcessId;
        Text = "SpeakMode owned native fixture " + fixture;
        StartPosition = FormStartPosition.Manual; Location = new Point(150, 100); Size = new Size(680, 350);
        shutdownTimer = new Timer { Interval = 100 };
        var deadline = DateTime.UtcNow.AddSeconds(30);
        shutdownTimer.Tick += (sender, args) => {
            if (closeRequested || IsDisposed || Disposing) return;
            if (File.Exists(Path.Combine(directory, fixture + "-exit")) || DateTime.UtcNow > deadline) {
                closeRequested = true;
                shutdownTimer.Stop();
                Close();
            }
        };
        Shown += (sender, args) => {
            string cls = "EDIT";
            uint style = 0x50210044;
            if (kind == "rich") { if (LoadLibraryW("msftedit.dll") == IntPtr.Zero) throw new InvalidOperationException("fixture rich edit DLL unavailable"); cls = "RICHEDIT50W"; }
            if (kind == "readonly") style |= 0x0800;
            if (kind == "password") style = 0x50210020;
            editor = CreateWindowExW(0, cls, "alpha | omega", style, 20, 20, 620, 150, Handle, IntPtr.Zero, IntPtr.Zero, IntPtr.Zero);
            if (editor == IntPtr.Zero) throw new InvalidOperationException("fixture edit creation failed");
            second = CreateWindowExW(0, "EDIT", "other owned input", 0x50210044, 20, 200, 620, 70, Handle, IntPtr.Zero, IntPtr.Zero, IntPtr.Zero);
            if (second == IntPtr.Zero) throw new InvalidOperationException("fixture alternate edit creation failed");
            SetForegroundWindow(Handle); SetFocus(editor);
            SendMessageW(editor, 0x00B1, new IntPtr(6), new IntPtr(7));
            // Only an already-foreground owned fixture grants permission back
            // to its specific parent probe. Never ASFW_ANY, simulated Alt or
            // AttachThreadInput; denied foreground activation remains a failure.
            bool delegated = GetForegroundWindow() == Handle && probeProcessId != 0 && AllowSetForegroundWindow(probeProcessId);
            string metadata = "{\"processId\":" + System.Diagnostics.Process.GetCurrentProcess().Id + ",\"hwnd\":" + Handle.ToInt64() + ",\"editor\":" + editor.ToInt64() + ",\"second\":" + second.ToInt64() + ",\"fixtureId\":\"" + fixture + "\",\"kind\":\"" + kind + "\",\"parentForegroundPermissionGranted\":" + delegated.ToString().ToLowerInvariant() + "}";
            string staging = Path.Combine(directory, fixture + "-ready.tmp");
            using (var stream = new FileStream(staging, FileMode.CreateNew, FileAccess.Write, FileShare.None))
            using (var writer = new StreamWriter(stream)) { writer.Write(metadata); }
            // A rename publishes complete metadata; the parent never observes
            // a partial JSON file. Existing run artifacts are never replaced.
            File.Move(staging, Path.Combine(directory, fixture + "-ready.json"));
            if (!closeRequested && !IsDisposed) shutdownTimer.Start();
        };
    }
    protected override void Dispose(bool disposing) {
        // A standalone Timer is not in the Form's component container.
        // Stop and release it before the window handle is disposed so no
        // queued shutdown tick can call Close() on an already closed fixture.
        if (disposing && shutdownTimer != null) {
            closeRequested = true;
            shutdownTimer.Stop();
            shutdownTimer.Dispose();
            shutdownTimer = null;
        }
        base.Dispose(disposing);
    }
    protected override void WndProc(ref Message msg) {
        // Parent can explicitly restore focus only to this fixture control.
        if (msg.Msg == 0x8031 && editor != IntPtr.Zero) { SetFocus(editor); msg.Result = new IntPtr(1); return; }
        if (msg.Msg == 0x8032 && second != IntPtr.Zero) { SetFocus(second); msg.Result = new IntPtr(1); return; }
        base.WndProc(ref msg);
    }
}
'@ -ReferencedAssemblies System.Windows.Forms,System.Drawing
$speakFixture = New-Object SpeakNativeFixture($Kind,$FixtureId,$speakScratch,[uint32]$ProbeProcessId)
[System.Windows.Forms.Application]::Run($speakFixture)
