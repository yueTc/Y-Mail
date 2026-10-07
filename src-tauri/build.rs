fn main() {
    // 图标（窗口图标、托盘图标、任务栏图标）是编译期打进 exe 资源段的；
    // cargo 默认只跟踪本文件和 tauri.conf.json，换了 icons 里的图片不会重编译，
    // 结果就是「图标改了但界面上还是旧图标」。这里显式声明，改图标后自动重编。
    println!("cargo:rerun-if-changed=icons");
    tauri_build::build();
}
