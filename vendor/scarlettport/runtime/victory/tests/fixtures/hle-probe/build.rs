fn main() {
    println!("cargo:rustc-link-arg-cdylib=/DELAYLOAD:victory-probe-delayed.dll");
    println!("cargo:rustc-link-lib=delayimp");
}
