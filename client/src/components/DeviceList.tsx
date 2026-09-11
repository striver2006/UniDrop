import React from "react";
import { Laptop, Monitor, Terminal, Send } from "lucide-react";
import { OnlineDevice } from "../types";

interface DeviceListProps {
  devices: OnlineDevice[];
  onSendToDevice: (device: OnlineDevice) => void;
}

export const DeviceList: React.FC<DeviceListProps> = ({ devices = [], onSendToDevice }) => {
  const safeDevices = Array.isArray(devices) ? devices : [];

  const getOSIcon = (os: string) => {
    switch ((os || "").toLowerCase()) {
      case "windows":
        return <Monitor className="w-5 h-5 text-blue-400" />;
      case "macos":
        return <Laptop className="w-5 h-5 text-zinc-300" />;
      default:
        return <Terminal className="w-5 h-5 text-emerald-400" />;
    }
  };

  if (safeDevices.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center py-10 text-slate-400 text-sm">
        <div className="w-12 h-12 rounded-full bg-slate-800 flex items-center justify-center mb-3">
          <Laptop className="w-6 h-6 text-slate-500" />
        </div>
        <p>暂无其他在线设备</p>
        <p className="text-xs text-slate-500 mt-1">在其他设备打开 UniDrop 并使用相同账号接入</p>
      </div>
    );
  }

  return (
    <div className="space-y-2">
      {safeDevices.map((device) => (
        <div
          key={device.device_id}
          className="flex items-center justify-between p-3 rounded-xl bg-slate-800/80 hover:bg-slate-800 border border-slate-700/60 transition duration-150"
        >
          <div className="flex items-center space-x-3">
            <div className="p-2 rounded-lg bg-slate-700/50">
              {getOSIcon(device.os_type)}
            </div>
            <div>
              <h4 className="text-sm font-medium text-slate-200">{device.hostname}</h4>
              <div className="flex items-center space-x-2 text-xs text-slate-400">
                <span className="w-1.5 h-1.5 rounded-full bg-emerald-400 animate-pulse" />
                <span>{device.os_type.toUpperCase()}</span>
                <span>•</span>
                <span>v{device.app_version}</span>
              </div>
            </div>
          </div>
          <button
            onClick={() => onSendToDevice(device)}
            className="flex items-center space-x-1.5 px-3 py-1.5 rounded-lg bg-teal-600/20 hover:bg-teal-600/30 text-teal-400 text-xs font-medium transition duration-150"
          >
            <Send className="w-3.5 h-3.5" />
            <span>发送</span>
          </button>
        </div>
      ))}
    </div>
  );
};
