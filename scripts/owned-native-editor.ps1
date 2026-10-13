param(
    [Parameter(Mandatory=$true)][string]$FixtureId,
    [Parameter(Mandatory=$true)][ValidateSet('edit','rich','readonly','password')][string]$Kind,
    [Parameter(Mandatory=$true)][string]$ScratchDir,
    [ValidateRange(1,2147483647)][int]$ProbeProcessId,
    [switch]$NativeMenu,
    [switch]$PhysicalAlt,
    [ValidateRange(5,180)][int]$LifetimeSeconds = 30
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
    [DllImport("user32.dll")] static extern IntPtr CreateMenu();
    [DllImport("user32.dll")] static extern IntPtr CreatePopupMenu();
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern bool AppendMenuW(IntPtr menu, uint flags, UIntPtr item, string text);
    [DllImport("user32.dll")] static extern bool SetMenu(IntPtr hwnd, IntPtr menu);
    [DllImport("user32.dll")] static extern bool DrawMenuBar(IntPtr hwnd);
    [DllImport("user32.dll")] static extern bool DestroyMenu(IntPtr menu);
    [DllImport("user32.dll")] static extern bool EndMenu();
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern bool SetWindowTextW(IntPtr hwnd, string text);
    private readonly string kind, fixture, directory;
    private readonly uint probeProcessId;
    private IntPtr editor, second;
    private Form alternate;
    private Timer shutdownTimer;
    private bool closeRequested;
    private readonly bool nativeMenu, physicalAlt;
    private IntPtr fixtureMenu;
    private int menuEntries, keyMenuRequests;
    private Label instructions;
    public SpeakNativeFixture(string kind, string fixture, string directory, uint probeProcessId, bool nativeMenu, bool physicalAlt, int lifetimeSeconds) {
        this.kind = kind; this.fixture = fixture; this.directory = directory;
        this.probeProcessId = probeProcessId;
        this.nativeMenu = nativeMenu; this.physicalAlt = physicalAlt;
        Text = "SpeakMode owned native fixture " + fixture;
        StartPosition = FormStartPosition.Manual; Location = new Point(150, 100); Size = new Size(680, 350);
        shutdownTimer = new Timer { Interval = 100 };
        var deadline = DateTime.UtcNow.AddSeconds(lifetimeSeconds);
        shutdownTimer.Tick += (sender, args) => {
            if (closeRequested || IsDisposed || Disposing) return;
            if (physicalAlt && instructions != null && File.Exists(Path.Combine(directory, fixture + "-physical-ready"))) {
                instructions.Text = "Press Right Alt twice here, two seconds apart. Keep this window active.";
            }
            if (File.Exists(Path.Combine(directory, fixture + "-exit")) || DateTime.UtcNow > deadline) {
                closeRequested = true;
                shutdownTimer.Stop();
                Close();
            }
        };
        Shown += (sender, args) => {
            if (nativeMenu) {
                fixtureMenu = CreateMenu();
                IntPtr popup = CreatePopupMenu();
                if (fixtureMenu == IntPtr.Zero || popup == IntPtr.Zero) throw new InvalidOperationException("fixture native menu creation failed");
                if (!AppendMenuW(popup, 0, new UIntPtr(1001), "Synthetic test item") ||
                    !AppendMenuW(fixtureMenu, 0x0010, new UIntPtr(unchecked((ulong)popup.ToInt64())), "&File") ||
                    !SetMenu(Handle, fixtureMenu) || !DrawMenuBar(Handle)) throw new InvalidOperationException("fixture native menu setup failed");
            }
            string cls = "EDIT";
            uint style = 0x50210044;
            if (kind == "rich") { if (LoadLibraryW("msftedit.dll") == IntPtr.Zero) throw new InvalidOperationException("fixture rich edit DLL unavailable"); cls = "RICHEDIT50W"; }
            if (kind == "readonly") style |= 0x0800;
            if (kind == "password") style = 0x50210020;
            editor = CreateWindowExW(0, cls, "alpha | omega", style, 20, 20, 620, 150, Handle, IntPtr.Zero, IntPtr.Zero, IntPtr.Zero);
            if (editor == IntPtr.Zero) throw new InvalidOperationException("fixture edit creation failed");
            second = CreateWindowExW(0, "EDIT", "other owned input", 0x50210044, 20, 200, 620, 70, Handle, IntPtr.Zero, IntPtr.Zero, IntPtr.Zero);
            if (second == IntPtr.Zero) throw new InvalidOperationException("fixture alternate edit creation failed");
            alternate = new Form { Text = "SpeakMode owned alternate " + fixture,
                StartPosition = FormStartPosition.Manual, Location = new Point(220, 180), Size = new Size(480, 180) };
            alternate.Controls.Add(new TextBox { Text = "other owned window", Dock = DockStyle.Fill, Multiline = true });
            IntPtr alternateHandle = alternate.Handle;
            if (nativeMenu) {
                instructions = new Label { Location = new Point(20, 274), Size = new Size(620, 35),
                    Text = physicalAlt ? "Wait until the probe is ready. Do not press keys yet." : "Owned synthetic fixture. No microphone or cloud requests." };
                Controls.Add(instructions);
            }
            SetForegroundWindow(Handle); SetFocus(editor);
            SendMessageW(editor, 0x00B1, new IntPtr(6), new IntPtr(7));
            // Only an already-foreground owned fixture grants permission back
            // to its specific parent probe. Never ASFW_ANY, simulated Alt or
            // AttachThreadInput; denied foreground activation remains a failure.
            bool delegated = GetForegroundWindow() == Handle && probeProcessId != 0 && AllowSetForegroundWindow(probeProcessId);
            string metadata = "{\"processId\":" + System.Diagnostics.Process.GetCurrentProcess().Id + ",\"hwnd\":" + Handle.ToInt64() + ",\"editor\":" + editor.ToInt64() + ",\"second\":" + second.ToInt64() + ",\"alternate\":" + alternateHandle.ToInt64() + ",\"fixtureId\":\"" + fixture + "\",\"kind\":\"" + kind + "\",\"nativeMenu\":" + nativeMenu.ToString().ToLowerInvariant() + ",\"parentForegroundPermissionGranted\":" + delegated.ToString().ToLowerInvariant() + "}";
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
        if (disposing && fixtureMenu != IntPtr.Zero) {
            SetMenu(Handle, IntPtr.Zero);
            DestroyMenu(fixtureMenu);
            fixtureMenu = IntPtr.Zero;
        }
        if (disposing && alternate != null) { alternate.Dispose(); alternate = null; }
        base.Dispose(disposing);
    }
    protected override void WndProc(ref Message msg) {
        // Parent can explicitly restore focus only to this fixture control.
        if (msg.Msg == 0x8031 && editor != IntPtr.Zero) { SetFocus(editor); msg.Result = new IntPtr(1); return; }
        if (msg.Msg == 0x8032 && second != IntPtr.Zero) { SetFocus(second); msg.Result = new IntPtr(1); return; }
        if (msg.Msg == 0x8033) { msg.Result = new IntPtr(menuEntries); return; }
        if (msg.Msg == 0x8034 && editor != IntPtr.Zero) {
            // This reset touches only the known synthetic child. It is fixture
            // setup, never a production focus/menu recovery implementation.
            EndMenu();
            SendMessageW(Handle, 0x001F, IntPtr.Zero, IntPtr.Zero);
            SetWindowTextW(editor, "alpha | omega");
            SetFocus(editor);
            SendMessageW(editor, 0x00B1, new IntPtr(6), new IntPtr(7));
            msg.Result = new IntPtr(1); return;
        }
        if (msg.Msg == 0x8035) { msg.Result = new IntPtr(keyMenuRequests); return; }
        if (msg.Msg == 0x8036 && alternate != null) {
            alternate.Show(); alternate.Activate(); SetForegroundWindow(alternate.Handle);
            msg.Result = new IntPtr(1); return;
        }
        if (nativeMenu && msg.Msg == 0x0211) menuEntries++;
        if (nativeMenu && msg.Msg == 0x0112 && (msg.WParam.ToInt64() & 0xfff0) == 0xf100) keyMenuRequests++;
        base.WndProc(ref msg);
    }
}
'@ -ReferencedAssemblies System.Windows.Forms,System.Drawing
$speakFixture = New-Object SpeakNativeFixture($Kind,$FixtureId,$speakScratch,[uint32]$ProbeProcessId,[bool]$NativeMenu,[bool]$PhysicalAlt,$LifetimeSeconds)
[System.Windows.Forms.Application]::Run($speakFixture)
