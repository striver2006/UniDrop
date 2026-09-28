package com.unidrop.unidrop_mobile

import android.Manifest
import android.annotation.SuppressLint
import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothManager
import android.content.pm.PackageManager
import android.os.Build
import android.provider.Settings
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel

class MainActivity : FlutterActivity() {
    // 授权弹窗挂起期间的通道应答；Activity 重建（罕见，configChanges 已
    // 覆盖常规旋转）会丢掉它，Dart 侧超时后走机型兜底，无需恢复。
    private var pendingNameResult: MethodChannel.Result? = null

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        // 上报对端 roster 的真实设备名（「设置→关于手机」里的设备名称），
        // device_info_plus 的 model 只是机型（"Mi 10"）。取数链与鸿蒙端同构：
        // 1. 蓝牙本机名——**必须放链首**：国内 ROM（MIUI/HyperOS）的用户
        //    设备名只同步到蓝牙名，Settings.Global 的 device_name 对 App
        //    可读但停在默认值（实测 Mi 10："Mi 10"），先读它永远拿不到
        //    用户改过的名字；类原生 ROM 的设备名与蓝牙名默认同步，蓝牙
        //    优先同样正确。Android 12+ 读它需 BLUETOOTH_CONNECT 运行时权限。
        // 2. Settings 的 device_name / bluetooth_name（无蓝牙设备的兜底）
        // 3. Build.MODEL 兜底（拒绝授权时；改名入口仍在设置页 device_name）
        MethodChannel(flutterEngine.dartExecutor.binaryMessenger, "unidrop/device_name")
            .setMethodCallHandler { call, result ->
                if (call.method == "getDeviceName") resolveDeviceName(result)
                else result.notImplemented()
            }
    }

    private fun resolveDeviceName(result: MethodChannel.Result) {
        val adapter = bluetoothAdapter()
        if (adapter == null) {
            result.success(settingsDeviceName() ?: Build.MODEL)
            return
        }
        // Android 12 以下：老 BLUETOOTH 权限安装期自动授予，直接读。
        if (Build.VERSION.SDK_INT < 31 ||
            checkSelfPermission(Manifest.permission.BLUETOOTH_CONNECT) == PackageManager.PERMISSION_GRANTED
        ) {
            result.success(bluetoothName(adapter) ?: settingsDeviceName() ?: Build.MODEL)
            return
        }
        // 未授权：本调用挂起等弹窗结果；弹窗期间到达的重复调用（Dart 超时
        // 重试）直接回兜底名，不叠加等待。
        if (pendingNameResult != null) {
            result.success(settingsDeviceName() ?: Build.MODEL)
            return
        }
        pendingNameResult = result
        requestPermissions(arrayOf(Manifest.permission.BLUETOOTH_CONNECT), REQ_BT_CONNECT)
    }

    private fun settingsDeviceName(): String? =
        (Settings.Global.getString(contentResolver, "device_name")
            ?: Settings.Secure.getString(contentResolver, "bluetooth_name"))
            ?.takeIf { it.isNotBlank() }

    // 蓝牙关闭时名字也能读（持久在配置里）；权限被收走时返回 null。
    @SuppressLint("MissingPermission")
    private fun bluetoothName(adapter: BluetoothAdapter): String? = try {
        adapter.name?.takeIf { it.isNotBlank() }
    } catch (_: SecurityException) {
        null
    }

    private fun bluetoothAdapter(): BluetoothAdapter? =
        (getSystemService(BLUETOOTH_SERVICE) as BluetoothManager).adapter

    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<String>,
        grantResults: IntArray,
    ) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode != REQ_BT_CONNECT) return
        val pending = pendingNameResult ?: return
        pendingNameResult = null
        // 拒绝不算错误：回 Settings/机型名，改名入口仍在（设置页 device_name）。
        if (grantResults.firstOrNull() == PackageManager.PERMISSION_GRANTED) {
            pending.success(bluetoothAdapter()?.let { bluetoothName(it) }
                ?: settingsDeviceName() ?: Build.MODEL)
        } else {
            pending.success(settingsDeviceName() ?: Build.MODEL)
        }
    }

    companion object {
        private const val REQ_BT_CONNECT = 4201
    }
}
