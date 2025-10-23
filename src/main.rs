use clap::{Parser, Subcommand};
use hound::{WavReader, WavWriter, SampleFormat, WavSpec};
use std::fs::File;
use std::io::{self, Write, BufWriter, BufReader, BufRead};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(name = "wav2pwl")]
#[command(about = "Convert between WAV and SPICE PWL formats", long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Convert WAV file to PWL format
    Wav2pwl {
        /// Input WAV file
        #[arg(short, long)]
        input: PathBuf,

        /// Output PWL file
        #[arg(short, long)]
        output: PathBuf,

        /// Voltage scale (peak voltage value, default: 1.0)
        #[arg(short, long, default_value_t = 1.0)]
        voltage_scale: f64,

        /// Decimation factor (output every Nth sample, default: 1 = no decimation)
        #[arg(short, long, default_value_t = 1)]
        decimate: usize,
    },
    /// Watch a PWL file and convert to WAV on changes
    Watch {
        /// Input PWL file to watch
        #[arg(short, long)]
        input: PathBuf,

        /// Output WAV file
        #[arg(short, long)]
        output: PathBuf,

        /// Sample rate for output WAV (default: 44100 Hz)
        #[arg(short, long, default_value_t = 44100)]
        sample_rate: u32,

        /// Voltage scale (how to scale voltage to samples, default: 1.0)
        #[arg(short, long, default_value_t = 1.0)]
        voltage_scale: f64,

        /// Column name or index to extract (e.g., "out" or "5"). Defaults to "out" if available.
        #[arg(short, long)]
        column: Option<String>,
    },
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    match args.command {
        Commands::Wav2pwl { input, output, voltage_scale, decimate } => {
            wav_to_pwl(&input, &output, voltage_scale, decimate)?;
        }
        Commands::Watch { input, output, sample_rate, voltage_scale, column } => {
            watch_pwl_to_wav(&input, &output, sample_rate, voltage_scale, column)?;
        }
    }

    Ok(())
}

fn wav_to_pwl(input: &PathBuf, output: &PathBuf, voltage_scale: f64, decimate: usize) -> std::result::Result<(), Box<dyn std::error::Error>> {
    // Validate decimation
    if decimate == 0 {
        return Err("Decimation factor must be at least 1".into());
    }

    // Open the WAV file
    let mut reader = WavReader::open(input)?;
    let spec = reader.spec();

    println!("Reading WAV file: {:?}", input);
    println!("  Sample rate: {} Hz", spec.sample_rate);
    println!("  Channels: {}", spec.channels);
    println!("  Bits per sample: {}", spec.bits_per_sample);
    println!("  Sample format: {:?}", spec.sample_format);
    println!("  Decimation: {} (effective rate: {} Hz)", decimate, spec.sample_rate / decimate as u32);

    if spec.channels > 1 {
        println!("  Note: Using only first channel for mono output");
    }

    // Calculate time step (accounting for decimation)
    let sample_rate = spec.sample_rate as f64;
    let time_step = decimate as f64 / sample_rate;

    // Open output file
    let output_file = File::create(output)?;
    let mut writer = BufWriter::new(output_file);

    println!("Writing PWL file: {:?}", output);

    // Process samples based on format
    match spec.sample_format {
        SampleFormat::Float => {
            write_pwl_float(&mut reader, &mut writer, time_step, voltage_scale, decimate, spec.channels)?;
        }
        SampleFormat::Int => {
            let max_value = (1i64 << (spec.bits_per_sample - 1)) as f64;
            write_pwl_int(&mut reader, &mut writer, time_step, voltage_scale, max_value, decimate, spec.channels)?;
        }
    }

    println!("Conversion complete!");

    Ok(())
}

fn write_pwl_float<R: std::io::Read>(
    reader: &mut WavReader<R>,
    writer: &mut BufWriter<File>,
    time_step: f64,
    voltage_scale: f64,
    decimate: usize,
    channels: u16,
) -> io::Result<()> {
    let channels = channels as usize;
    let mut frame_count = 0usize;
    let mut output_count = 0usize;

    for (i, sample) in reader.samples::<f32>().enumerate() {
        let sample = sample.map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        // Only process first channel of each frame
        if i % channels == 0 {
            // Only output every Nth frame
            if frame_count % decimate == 0 {
                let time = output_count as f64 * time_step;
                let voltage = sample as f64 * voltage_scale;
                writeln!(writer, "{:.6e}, {:.6e}", time, voltage)?;
                output_count += 1;
            }
            frame_count += 1;
        }
    }
    Ok(())
}

fn write_pwl_int<R: std::io::Read>(
    reader: &mut WavReader<R>,
    writer: &mut BufWriter<File>,
    time_step: f64,
    voltage_scale: f64,
    max_value: f64,
    decimate: usize,
    channels: u16,
) -> io::Result<()> {
    let channels = channels as usize;
    let mut frame_count = 0usize;
    let mut output_count = 0usize;

    for (i, sample) in reader.samples::<i32>().enumerate() {
        let sample = sample.map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        // Only process first channel of each frame
        if i % channels == 0 {
            // Only output every Nth frame
            if frame_count % decimate == 0 {
                let time = output_count as f64 * time_step;
                // Normalize to -1.0 to 1.0, then scale by voltage_scale
                let voltage = (sample as f64 / max_value) * voltage_scale;
                writeln!(writer, "{:.6e}, {:.6e}", time, voltage)?;
                output_count += 1;
            }
            frame_count += 1;
        }
    }
    Ok(())
}

fn pwl_to_wav(input: &PathBuf, output: &PathBuf, sample_rate: u32, voltage_scale: f64, column: Option<String>) -> std::result::Result<(), Box<dyn std::error::Error>> {
    println!("Reading file: {:?}", input);

    // Read the file
    let file = File::open(input)?;
    let reader = BufReader::new(file);
    let mut lines = reader.lines();

    // Try to read first line to detect header
    let first_line = if let Some(Ok(line)) = lines.next() {
        line
    } else {
        return Err("Empty file".into());
    };

    // Check if first line is a header (contains non-numeric values)
    let has_header = first_line.split_whitespace()
        .any(|part| !part.contains(',') && part.parse::<f64>().is_err() && part != "time");

    let (header_cols, column_index): (Option<Vec<String>>, usize) = if has_header {
        let cols: Vec<String> = first_line.split_whitespace().map(|s| s.to_string()).collect();

        // Determine which column to use
        let col_idx = if let Some(ref col_spec) = column {
            // Try parsing as index first
            if let Ok(idx) = col_spec.parse::<usize>() {
                idx
            } else {
                // Try finding by name
                cols.iter().position(|c| c == col_spec)
                    .ok_or_else(|| format!("Column '{}' not found. Available: {}", col_spec, cols.join(", ")))?
            }
        } else {
            // Default to "out" column if it exists
            cols.iter().position(|c| c == "out").unwrap_or_else(|| {
                // If no "out" column, prompt user
                println!("Available columns:");
                for (i, col) in cols.iter().enumerate() {
                    println!("  [{}] {}", i, col);
                }
                println!("Using column 0 (time). Specify -c <name|index> to select another column.");
                0
            })
        };

        if col_idx >= cols.len() {
            return Err(format!("Column index {} out of range (have {} columns)", col_idx, cols.len()).into());
        }

        println!("  Extracting column: {} ({})", col_idx, cols[col_idx]);
        (Some(cols), col_idx)
    } else {
        // No header, use column specification or default to 1
        let col_idx = if let Some(ref col_spec) = column {
            col_spec.parse::<usize>()
                .map_err(|_| format!("No header found. Column must be numeric index, not '{}'", col_spec))?
        } else {
            1 // Default to column 1
        };
        println!("  Extracting column: {}", col_idx);
        (None, col_idx)
    };

    let mut samples: Vec<(f64, f64)> = Vec::new();

    // Process data lines
    let data_lines = if has_header {
        lines // Continue from second line
    } else {
        // Need to process first line too, recreate iterator
        let file = File::open(input)?;
        let reader = BufReader::new(file);
        reader.lines()
    };

    for line in data_lines {
        let line = line?;
        let line = line.trim();

        // Skip empty lines and comments
        if line.is_empty() || line.starts_with('*') || line.starts_with(';') {
            continue;
        }

        // Try to detect format: comma-separated or space-separated
        let (time, voltage) = if line.contains(',') {
            // CSV format: comma-separated columns
            let parts: Vec<&str> = line.split(',').collect();
            if parts.is_empty() {
                continue;
            }

            // Always use first column as time
            let time = parts[0].trim().parse::<f64>()?;

            // Use specified column or default
            let data_col = if header_cols.is_some() {
                column_index
            } else {
                column.as_ref()
                    .and_then(|s| s.parse::<usize>().ok())
                    .unwrap_or(1) // Default to column 1 if not specified
            };

            if data_col >= parts.len() {
                return Err(format!("Column {} not found (line has {} columns)", data_col, parts.len()).into());
            }

            let voltage = parts[data_col].trim().parse::<f64>()?;
            (time, voltage)
        } else {
            // SPICE format: space-separated columns
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.is_empty() {
                continue;
            }

            if column_index >= parts.len() {
                return Err(format!("Column {} not found (line has {} columns)", column_index, parts.len()).into());
            }

            // Column 0 is always time
            let time = parts[0].parse::<f64>()?;
            let voltage = parts[column_index].parse::<f64>()?;
            (time, voltage)
        };

        samples.push((time, voltage));
    }

    if samples.is_empty() {
        return Err("No valid samples found in PWL file".into());
    }

    println!("  Found {} PWL points", samples.len());

    // Determine the total duration
    let duration = samples.last().unwrap().0;
    let num_output_samples = (duration * sample_rate as f64).ceil() as usize;

    println!("  Duration: {:.6} seconds", duration);
    println!("  Output samples: {}", num_output_samples);

    // Create WAV spec
    let spec = WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };

    // Create WAV writer
    let mut writer = WavWriter::create(output, spec)?;

    println!("Writing WAV file: {:?}", output);

    // Interpolate and write samples
    let time_step = 1.0 / sample_rate as f64;

    for i in 0..num_output_samples {
        let t = i as f64 * time_step;

        // Find the surrounding PWL points for interpolation
        let voltage = interpolate_pwl(&samples, t);

        // Convert voltage to 16-bit PCM sample
        let normalized = voltage / voltage_scale;
        let sample = (normalized * 32767.0).clamp(-32768.0, 32767.0) as i16;

        writer.write_sample(sample)?;
    }

    writer.finalize()?;
    println!("Conversion complete!");

    Ok(())
}

fn interpolate_pwl(samples: &[(f64, f64)], t: f64) -> f64 {
    if t <= samples[0].0 {
        return samples[0].1;
    }

    if t >= samples.last().unwrap().0 {
        return samples.last().unwrap().1;
    }

    // Binary search for the right interval
    let mut left = 0;
    let mut right = samples.len() - 1;

    while left < right - 1 {
        let mid = (left + right) / 2;
        if samples[mid].0 <= t {
            left = mid;
        } else {
            right = mid;
        }
    }

    // Linear interpolation
    let (t0, v0) = samples[left];
    let (t1, v1) = samples[right];

    let alpha = (t - t0) / (t1 - t0);
    v0 + alpha * (v1 - v0)
}

fn watch_pwl_to_wav(input: &PathBuf, output: &PathBuf, sample_rate: u32, voltage_scale: f64, column: Option<String>) -> std::result::Result<(), Box<dyn std::error::Error>> {
    println!("Watching file: {:?}", input);
    println!("Will convert to WAV: {:?}", output);
    if let Some(ref col) = column {
        println!("Extracting column: {}", col);
    }
    println!("Waiting for file to be created/updated...");
    println!("Press Ctrl+C to stop watching...");
    println!();

    // Do initial conversion if file exists
    if input.exists() {
        println!("File found, converting...");
        match pwl_to_wav(input, output, sample_rate, voltage_scale, column.clone()) {
            Ok(_) => {
                println!("Conversion successful!");
                // Delete the input file after successful conversion
                if let Err(e) = std::fs::remove_file(input) {
                    eprintln!("Warning: Could not delete input file: {}", e);
                } else {
                    println!("Input file deleted, waiting for next export...");
                }
            }
            Err(e) => eprintln!("Error during initial conversion: {}", e),
        }
        println!();
    }

    // Use simple polling instead of file watcher for better reliability
    // This is more reliable for detecting file creation after deletion
    loop {
        std::thread::sleep(Duration::from_millis(100));

        if input.exists() {
            // Wait for file to be fully written
            println!("File detected, waiting for write to complete...");

            // Check file stability by comparing size over time
            let mut stable_count = 0;
            let mut last_size = 0u64;

            // Check up to 20 times (4 seconds total)
            for _ in 0..20 {
                std::thread::sleep(Duration::from_millis(200));

                if let Ok(metadata) = std::fs::metadata(input) {
                    let current_size = metadata.len();
                    if current_size == last_size && current_size > 0 {
                        stable_count += 1;
                        // Require 3 consecutive stable checks (600ms of stability)
                        if stable_count >= 3 {
                            break;
                        }
                    } else {
                        stable_count = 0;
                        last_size = current_size;
                    }
                } else {
                    // File disappeared, break and wait for it again
                    break;
                }
            }

            // Additional safety wait
            println!("File appears stable, waiting additional 500ms for safety...");
            std::thread::sleep(Duration::from_millis(500));

            // Double-check file still exists after waiting
            if !input.exists() {
                continue;
            }

            println!("Converting...");
            match pwl_to_wav(input, output, sample_rate, voltage_scale, column.clone()) {
                Ok(_) => {
                    println!("Conversion successful!");
                    // Delete the input file after successful conversion
                    if let Err(e) = std::fs::remove_file(input) {
                        eprintln!("Warning: Could not delete input file: {}", e);
                    } else {
                        println!("Input file deleted, waiting for next export...");
                    }
                }
                Err(e) => eprintln!("Error during conversion: {}", e),
            }
            println!();
        }
    }
}
