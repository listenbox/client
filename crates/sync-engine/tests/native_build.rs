// Keep the process-launch regression in a small executable: copying and starting
// the whole media test harness repeatedly adds unrelated native loader work.
#[allow(dead_code)]
#[path = "../../../tools/native-ffmpeg.rs"]
mod build_recipe;
