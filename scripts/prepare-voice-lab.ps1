param([Parameter(Mandatory=$true)][string]$OutputDirectory)
$ErrorActionPreference='Stop'
$labDir=[IO.Path]::GetFullPath($OutputDirectory)
if (-not [IO.Path]::IsPathRooted($OutputDirectory) -or (Test-Path -LiteralPath $labDir)) { throw 'Use a new absolute experiment directory.' }
New-Item -ItemType Directory -Path $labDir | Out-Null
New-Item -ItemType Directory -Path (Join-Path $labDir 'fixtures') | Out-Null
$labVoice=New-Object -ComObject SAPI.SpVoice
$labToken=@($labVoice.GetVoices() | Where-Object {$_.GetDescription() -eq 'Microsoft Huihui Desktop - Chinese (Simplified)'})
if ($labToken.Count -ne 1) { throw 'Required local offline Huihui TTS voice unavailable.' }
$labVoice.Voice=$labToken[0]
$labVoice.Rate=0
$labVoice.Volume=100
$labTexts=[ordered]@{
  A='今天用 SpeakMode 比较 Typeless 和 Superwhisper。音频采集用 WASAPI，重采样用 Rubato。'
  B='嗯，下周的开发安排，预算先五千，不对，是三千五百元，不是五千元。如果时间还没确认，先不要发邀请。我们可能周五让同学试用，但是时间还没确定。'
  C1='第一项，先检查麦克风。'
  C2='第二项，把测试结果保存到本地。最后一句是，不要删除原始录音。'
}
foreach ($labEntry in $labTexts.GetEnumerator()) {
  $labPath=Join-Path $labDir ('fixtures/'+$labEntry.Key+'.wav')
  $labStream=New-Object -ComObject SAPI.SpFileStream
  try {
    # SAFT16kHz16BitMono. The subsequent Python step validates the actual header.
    $labStream.Format.Type=18
    $labStream.Open($labPath,3,$false)
    $labVoice.AudioOutputStream=$labStream
    [void]$labVoice.Speak($labEntry.Value)
  } finally { $labStream.Close() }
}
$labTextPath=Join-Path $labDir 'fixture-texts.json'
[IO.File]::WriteAllText($labTextPath,($labTexts | ConvertTo-Json),[Text.UTF8Encoding]::new($false))
Write-Output ('Generated offline fixed TTS fixtures: '+$labDir)
