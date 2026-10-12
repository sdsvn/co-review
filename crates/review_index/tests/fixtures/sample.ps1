function Get-User {
    param([string]$Name)
    return $Name
}

class Logger {
    [void] Log([string]$Message) {
        Write-Output $Message
    }
}
