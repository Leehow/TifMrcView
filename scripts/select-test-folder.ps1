param([int]$TargetProcessId)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient
$root = [System.Windows.Automation.AutomationElement]::RootElement
$process = [System.Windows.Automation.PropertyCondition]::new(
    [System.Windows.Automation.AutomationElement]::ProcessIdProperty, $TargetProcessId)
$selector = [System.Windows.Automation.PropertyCondition]::new(
    [System.Windows.Automation.AutomationElement]::NameProperty, 'Select Folder')
for ($attempt = 0; $attempt -lt 30; $attempt++) {
    foreach ($window in $root.FindAll([System.Windows.Automation.TreeScope]::Children, $process)) {
        $button = $window.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $selector)
        if ($button -and $button.Current.IsEnabled) {
            $button.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke()
            Write-Output 'Invoked Select Folder on the test process'
            exit 0
        }
    }
    Start-Sleep -Milliseconds 400
}
throw 'Could not find the Select Folder button on the test application'
