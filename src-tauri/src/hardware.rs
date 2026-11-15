use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HardwareInfo {
    pub computer_model: String,
    pub motherboard: String,
    pub os_name: String,
    pub os_version: String,
    pub processor: String,
    pub cpu_cores: u32,
    pub cpu_threads: u32,
    pub cpu_clock_mhz: u32,
    pub cpu_temp_c: Option<f64>,
    pub gpu: Vec<GpuInfo>,
    pub memory_total_bytes: u64,
    pub memory_used_bytes: u64,
    pub drives: Vec<DriveInfo>,
    pub sensor_provider: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuInfo {
    pub name: String,
    pub driver_version: String,
    pub memory_bytes: Option<u64>,
    pub temperature_c: Option<f64>,
    pub load_percent: Option<f64>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveInfo {
    pub name: String,
    pub total_bytes: u64,
    pub free_bytes: u64,
}

pub fn read() -> Result<HardwareInfo, String> {
    use std::os::windows::process::CommandExt;

    let script = r#"
$ErrorActionPreference='SilentlyContinue'
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
$cpu=Get-CimInstance Win32_Processor | Select-Object -First 1
$cs=Get-CimInstance Win32_ComputerSystem
$os=Get-CimInstance Win32_OperatingSystem
$board=Get-CimInstance Win32_BaseBoard | Select-Object -First 1
$ram=Get-CimInstance Win32_PhysicalMemory | Measure-Object -Property Capacity -Sum
$drives=@(Get-CimInstance Win32_LogicalDisk -Filter 'DriveType=3' | ForEach-Object { [pscustomobject]@{name=[string]$_.DeviceID;totalBytes=[uint64]$_.Size;freeBytes=[uint64]$_.FreeSpace} })
$sensors=@()
try { $sensors=@(Get-CimInstance -Namespace root/LibreHardwareMonitor -ClassName Sensor) } catch {}
$cpuSensor=$sensors | Where-Object { $_.SensorType -eq 'Temperature' -and $_.Name -match 'CPU Package|CPU CCD|CPU Core' } | Select-Object -First 1
$gpus=@(Get-CimInstance Win32_VideoController | Where-Object { $_.Name } | ForEach-Object { [pscustomobject]@{name=[string]$_.Name;driverVersion=[string]$_.DriverVersion;memoryBytes=if($_.AdapterRAM){[uint64]$_.AdapterRAM}else{$null};temperatureC=$null;loadPercent=$null} })
$provider=$null
if($sensors.Count -gt 0){$provider='LibreHardwareMonitor'}
$smi=(Get-Command nvidia-smi.exe -ErrorAction SilentlyContinue).Source
if(-not $smi){$candidate=Join-Path $env:ProgramFiles 'NVIDIA Corporation\NVSMI\nvidia-smi.exe';if(Test-Path $candidate){$smi=$candidate}}
if($smi){
  $temps=@(& $smi --query-gpu=temperature.gpu --format=csv,noheader,nounits 2>$null)
  $loads=@(& $smi --query-gpu=utilization.gpu --format=csv,noheader,nounits 2>$null)
  for($i=0;$i -lt $gpus.Count;$i++){
    if($i -lt $temps.Count){$v=0.0;if([double]::TryParse(([string]$temps[$i]).Trim(),[ref]$v)){$gpus[$i].temperatureC=$v}}
    if($i -lt $loads.Count){$v=0.0;if([double]::TryParse(([string]$loads[$i]).Trim(),[ref]$v)){$gpus[$i].loadPercent=$v}}
  }
  $provider='NVIDIA driver'
}
foreach($g in $gpus){
  if($null -eq $g.temperatureC){$sensor=$sensors | Where-Object { $_.SensorType -eq 'Temperature' -and $_.Name -match 'GPU Core|GPU Temperature' } | Select-Object -First 1;if($sensor){$g.temperatureC=[double]$sensor.Value}}
  if($null -eq $g.loadPercent){$sensor=$sensors | Where-Object { $_.SensorType -eq 'Load' -and $_.Name -match 'GPU Core' } | Select-Object -First 1;if($sensor){$g.loadPercent=[double]$sensor.Value}}
}
$cpuName=[string]$cpu.Name
$model=[string]$cs.Model
if($model -match 'To Be Filled By O\.E\.M\.|System Product Name|Default string'){$model='Модель не указана производителем'}
$mb=(([string]$board.Manufacturer+' '+[string]$board.Product).Trim())
if($mb -match 'To Be Filled By O\.E\.M\.|Default string|^$'){$mb='Модель не указана производителем'}
$total=[uint64]$cs.TotalPhysicalMemory
$free=[uint64]$os.FreePhysicalMemory*1024
[pscustomobject]@{computerModel=$model;motherboard=$mb;osName=[string]$os.Caption;osVersion=[string]$os.Version;processor=$cpuName;cpuCores=[uint32]$cpu.NumberOfCores;cpuThreads=[uint32]$cpu.NumberOfLogicalProcessors;cpuClockMhz=[uint32]$cpu.MaxClockSpeed;cpuTempC=if($cpuSensor){[double]$cpuSensor.Value}else{$null};gpu=$gpus;memoryTotalBytes=$total;memoryUsedBytes=[Math]::Max([int64]0,([int64]$total-[int64]$free));drives=$drives;sensorProvider=$provider} | ConvertTo-Json -Depth 5 -Compress
"#;

    let windir = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
    let modules = std::path::PathBuf::from(windir)
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("Modules");
    let output = Command::new("powershell.exe")
        .env("PSModulePath", modules)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ])
        .creation_flags(0x08000000)
        .output()
        .map_err(|e| format!("Не удалось запустить сбор сведений Windows: {e}"))?;
    if !output.status.success() {
        return Err("Windows не смогла собрать сведения о компонентах ПК".into());
    }
    let mut info: HardwareInfo = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("Не удалось прочитать сведения Windows: {e}"))?;

    // NVAPI can read the GTX 750 Ti's on-card sensor even when nvidia-smi is absent.
    #[cfg(target_os = "windows")]
    if let Ok(adapters) = nvapi::PhysicalGpu::enumerate() {
        let mut got_temperature = false;
        for (gpu, adapter) in info.gpu.iter_mut().zip(adapters.iter()) {
            if gpu.temperature_c.is_none() {
                if let Ok(sensors) = adapter.thermal_settings(None) {
                    if let Some(sensor) = sensors.first() {
                        gpu.temperature_c = Some(sensor.current_temperature.0 as f64);
                        got_temperature = true;
                    }
                }
            }
        }
        if got_temperature {
            info.sensor_provider = Some("Драйвер NVIDIA NVAPI".into());
        }
    }
    Ok(info)
}
