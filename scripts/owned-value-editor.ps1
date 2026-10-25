# A purpose-built app under test, not a desktop automation script.
# UI actions are performed by the tester. Only synthetic text is used.
param(
    [Parameter(Mandatory=$true)][string]$Backend,
    [Parameter(Mandatory=$true)][string]$OutputDirectory,
    [string]$Executable
)
$ErrorActionPreference='Stop'
if (-not [IO.Path]::IsPathRooted($Backend) -or -not (Test-Path -LiteralPath $Backend -PathType Leaf)) { throw 'Existing absolute backend required' }
if (-not [IO.Path]::IsPathRooted($OutputDirectory) -or -not (Test-Path -LiteralPath $OutputDirectory -PathType Container)) { throw 'Existing absolute output directory required' }
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationProvider
Add-Type -AssemblyName UIAutomationTypes
$speakFixtureSource = @'
using System;
using System.IO;
using System.Diagnostics;
using System.Drawing;
using System.Windows.Forms;
using System.Windows.Automation;
using System.Windows.Automation.Provider;
public sealed class OwnedValueEditor : Control, IRawElementProviderSimple, IValueProvider {
    public bool ReadOnlyField, PasswordField;
    public string Content="alpha | omega";
    public int PasteCount;
    public OwnedValueEditor() { TabStop=true; BackColor=Color.White; SetStyle(ControlStyles.Selectable,true); }
    public ProviderOptions ProviderOptions { get { return ProviderOptions.ServerSideProvider; } }
    public IRawElementProviderSimple HostRawElementProvider { get { return AutomationInteropProvider.HostProviderFromHandle(Handle); } }
    public object GetPatternProvider(int id) { return id==ValuePatternIdentifiers.Pattern.Id ? this : null; }
    public object GetPropertyValue(int id) {
        if(id==AutomationElementIdentifiers.ControlTypeProperty.Id) return ControlType.Edit.Id;
        if(id==AutomationElementIdentifiers.IsKeyboardFocusableProperty.Id) return true;
        // UIA can invoke the provider on a COM worker. Control.Focused uses
        // thread-local GetFocus, so query it on the control's UI thread.
        if(id==AutomationElementIdentifiers.HasKeyboardFocusProperty.Id)
            return InvokeRequired ? (bool)Invoke(new Func<bool>(()=>Focused)) : Focused;
        if(id==AutomationElementIdentifiers.IsEnabledProperty.Id) return true;
        if(id==AutomationElementIdentifiers.IsPasswordProperty.Id) return PasswordField;
        if(id==AutomationElementIdentifiers.IsControlElementProperty.Id || id==AutomationElementIdentifiers.IsContentElementProperty.Id) return true;
        if(id==AutomationElementIdentifiers.NameProperty.Id) return "Owned ValuePattern-only editor";
        return null;
    }
    public bool IsReadOnly { get { return ReadOnlyField; } }
    public string Value { get { return Content; } }
    public void SetValue(string value) { if(ReadOnlyField) throw new InvalidOperationException("readonly"); Content=value; Invalidate(); }
    protected override void WndProc(ref Message message) {
        // Standard server provider registration, per Microsoft documentation.
        if(message.Msg==0x003D && unchecked((int)message.LParam.ToInt64())==AutomationInteropProvider.RootObjectId) {
            message.Result=AutomationInteropProvider.ReturnRawElementProvider(Handle,message.WParam,message.LParam,this); return;
        }
        base.WndProc(ref message);
    }
    protected override void OnGotFocus(EventArgs args) {
        base.OnGotFocus(args);
        AutomationInteropProvider.RaiseAutomationEvent(AutomationElementIdentifiers.AutomationFocusChangedEvent,this,
            new AutomationEventArgs(AutomationElementIdentifiers.AutomationFocusChangedEvent));
        Invalidate();
    }
    protected override void OnMouseDown(MouseEventArgs args) { base.OnMouseDown(args); Focus(); }
    protected override void OnKeyDown(KeyEventArgs args) {
        if(args.Control && args.KeyCode==Keys.V) {
            if(!ReadOnlyField && !PasswordField) { Content += Clipboard.GetText(); PasteCount++; Invalidate(); }
            args.SuppressKeyPress=true; return;
        }
        base.OnKeyDown(args);
    }
    protected override void OnPaint(PaintEventArgs args) {
        base.OnPaint(args);
        args.Graphics.DrawRectangle(Focused?Pens.RoyalBlue:Pens.Gray,0,0,Width-1,Height-1);
        args.Graphics.DrawString(PasswordField?"synthetic password field":Content,Font,Brushes.Black,new RectangleF(12,12,Width-24,Height-24));
    }
}
public static class OwnedValueWindow {
    [STAThread] public static void Main() {
        var settings=File.ReadAllLines(Path.Combine(AppDomain.CurrentDomain.BaseDirectory,"owned-value-editor.config"));
        if(settings.Length!=2) throw new InvalidOperationException("owned fixture config required");
        Run(settings[0],settings[1]);
    }
    [STAThread] public static void Run(string backend,string output) {
        Application.EnableVisualStyles();
        var form=new Form { Text="SpeakMode owned ValuePattern input test",Width=820,Height=430,StartPosition=FormStartPosition.CenterScreen };
        var editor=new OwnedValueEditor { Left=24,Top=80,Width=750,Height=140,Font=new Font("Microsoft YaHei UI",13) };
        var status=new Label { Left=24,Top=24,Width=750,Height=40,Text="Synthetic test only. No microphone, account or chat data." };
        form.Controls.Add(editor); form.Controls.Add(status);
        int sequence=0;
        foreach(string kind in new[]{"writable","readonly","password"}) {
            var current=kind;
            var button=new Button { Left=24+sequence*245,Top=240,Width=220,Height=38,Text="Test "+kind };
            sequence++;
            button.Click+=(sender,args)=>{
                editor.ReadOnlyField=current=="readonly";editor.PasswordField=current=="password";
                editor.Content="alpha | omega";editor.PasteCount=0;editor.Invalidate();
                var path=Path.Combine(output,Guid.NewGuid().ToString("N")+"-"+current+".json");
                var process=new Process { StartInfo=new ProcessStartInfo { FileName=backend,
                    Arguments=form.Handle.ToInt64()+" "+editor.Handle.ToInt64()+" "+Process.GetCurrentProcess().Id+" "+current+" \""+path+"\"",
                    UseShellExecute=false,CreateNoWindow=true } };
                editor.Focus();
                process.Start(); status.Text="Running "+current+". Keep this synthetic field focused.";
                var timer=new Timer { Interval=100 };
                timer.Tick+=(tickSender,tickArgs)=>{
                    if(!process.HasExited) return;
                    timer.Stop();timer.Dispose();
                    status.Text=current+" backend exit="+process.ExitCode+"; actual paste count="+editor.PasteCount+"; "+Path.GetFileName(path);
                    process.Dispose();
                };
                timer.Start();
            };
            form.Controls.Add(button);
        }
        Application.Run(form);
    }
}
'@
$speakFixtureArguments = @{ TypeDefinition=$speakFixtureSource; ReferencedAssemblies=@('System.Windows.Forms','System.Drawing','UIAutomationClient','UIAutomationProvider','UIAutomationTypes','WindowsBase') }
if($Executable) {
    if(-not [IO.Path]::IsPathRooted($Executable) -or (Test-Path -LiteralPath $Executable)) { throw 'Fresh absolute fixture executable required' }
    $speakFixtureArguments.OutputAssembly=$Executable
    $speakFixtureArguments.OutputType='WindowsApplication'
    Add-Type @speakFixtureArguments
    [IO.File]::WriteAllLines((Join-Path ([IO.Path]::GetDirectoryName($Executable)) 'owned-value-editor.config'),@($Backend,$OutputDirectory))
}else {
    Add-Type @speakFixtureArguments
    [OwnedValueWindow]::Run($Backend,$OutputDirectory)
}
