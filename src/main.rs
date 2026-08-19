use cpal::traits::{DeviceTrait,HostTrait};
fn main(){
    let host=cpal::default_host();
    let device=host.default_input_device().expect("no input device found");
    println!("using:{}",device);
    let config=device.default_input_config().expect("no default config");
    println!("sample rate:{}",config.sample_rate());
    println!("channels:{}",config.channels());
}
