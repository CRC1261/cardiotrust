use std::time::Duration;

use cardiotrust::core::{
    algorithm::{
        estimation::prediction::calculate_system_prediction,
        gpu::{derivation::DerivationKernel, prediction::PredictionKernel, GPU},
        refinement::derivation::calculate_step_derivatives,
    },
    config::Config,
    data::Data,
    model::Model,
    scenario::results::{Results, ResultsGPU},
};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

const VOXEL_SIZES: [f32; 4] = [1.0, 2.0, 2.5, 5.0];

fn run_benches(c: &mut Criterion) {
    let mut group = c.benchmark_group("GPU Derivation");
    prectiction_benches(&mut group);
    group.finish();
}

fn prectiction_benches(group: &mut criterion::BenchmarkGroup<criterion::measurement::WallTime>) {
    for voxel_size in VOXEL_SIZES.iter() {
        let config = setup_config(voxel_size);
        let (data, mut results, _gpu, results_gpu, prediction_kernel, derivation_kernel) =
            setup_inputs(&config);
        let mut results_from_gpu = results.clone();

        let number_of_voxels = results
            .model
            .as_ref()
            .unwrap()
            .spatial_description
            .voxels
            .count();
        group.throughput(criterion::Throughput::Elements(number_of_voxels as u64));
        for step in 0..data.simulation.measurements.num_steps() {
            calculate_system_prediction(
                &mut results.estimations,
                &results.model.as_ref().unwrap().functional_description,
                0,
                step,
            );
        }
        group.bench_function(BenchmarkId::new("cpu", voxel_size), |b| {
            b.iter(|| {
                for step in 0..data.simulation.measurements.num_steps() {
                    calculate_step_derivatives(
                        &mut results.derivatives,
                        &results.estimations,
                        &results.model.as_ref().unwrap().functional_description,
                        &config.algorithm,
                        step,
                        0,
                        results.estimations.measurements.num_sensors(),
                    );
                }
            })
        });
        for step in 0..data.simulation.measurements.num_steps() {
            results_gpu
                .estimations
                .step
                .write([step as i32].as_slice())
                .enq()
                .unwrap();
            prediction_kernel.execute();
        }
        group.bench_function(BenchmarkId::new("gpu", voxel_size), |b| {
            b.iter(|| {
                for step in 0..data.simulation.measurements.num_steps() {
                    results_gpu
                        .estimations
                        .step
                        .write([step as i32].as_slice())
                        .enq()
                        .unwrap();
                    derivation_kernel.execute();
                }
            })
        });
        group.bench_function(BenchmarkId::new("gpu_and_read", voxel_size), |b| {
            b.iter(|| {
                for step in 0..data.simulation.measurements.num_steps() {
                    results_gpu
                        .estimations
                        .step
                        .write([step as i32].as_slice())
                        .enq()
                        .unwrap();
                    derivation_kernel.execute();
                }
                results_from_gpu.update_from_gpu(&results_gpu);
            })
        });
    }
}

fn setup_config(voxel_size: &f32) -> Config {
    let samplerate_hz = 2000.0 * 2.5 / voxel_size;
    let mut config = Config::default();
    config.simulation.model.common.voxel_size_mm = *voxel_size;
    config.simulation.model.common.pathological = true;
    config.simulation.sample_rate_hz = samplerate_hz;
    config.algorithm.model.common.voxel_size_mm = *voxel_size;
    config.algorithm.learning_rate = 1e3;
    config.algorithm.freeze_delays = false;
    config.algorithm.freeze_gains = false;
    config.algorithm.batch_size = 0;
    config.algorithm.update_kalman_gain = true;
    config
}

fn setup_inputs(
    config: &Config,
) -> (
    Data,
    Results,
    GPU,
    ResultsGPU,
    PredictionKernel,
    DerivationKernel,
) {
    let simulation_config = &config.simulation;
    let data =
        Data::from_simulation_config(simulation_config).expect("Model parameters to be valid.");
    let model = Model::from_model_config(
        &config.algorithm.model,
        simulation_config.sample_rate_hz,
        simulation_config.duration_s,
    )
    .unwrap();
    let mut results = Results::new(
        config.algorithm.epochs,
        data.simulation.measurements.num_steps(),
        model.spatial_description.sensors.count(),
        model.spatial_description.voxels.count_states(),
        model.spatial_description.sensors.count_beats(),
        0,
        config.algorithm.batch_size,
        config.algorithm.optimizer,
    );
    results.model = Some(model);
    let gpu = GPU::new();
    let results_gpu = results.to_gpu(&gpu.queue);
    let prediction_kernel = PredictionKernel::new(
        &gpu,
        &results_gpu.estimations,
        &results_gpu.model,
        results
            .model
            .as_ref()
            .unwrap()
            .spatial_description
            .voxels
            .count_states() as i32,
        results
            .model
            .as_ref()
            .unwrap()
            .spatial_description
            .sensors
            .count() as i32,
        results.estimations.measurements.num_steps() as i32,
    );
    let actual_measurements = data.simulation.measurements.to_gpu(&gpu.queue);
    let derivation_kernel = DerivationKernel::new(
        &gpu,
        &results_gpu.estimations,
        &results_gpu.derivatives,
        &actual_measurements,
        &results_gpu.model,
        results
            .model
            .as_ref()
            .unwrap()
            .spatial_description
            .voxels
            .count_states() as i32,
        results
            .model
            .as_ref()
            .unwrap()
            .spatial_description
            .sensors
            .count() as i32,
        results.estimations.measurements.num_steps() as i32,
        &config.algorithm,
    );
    (
        data,
        results,
        gpu,
        results_gpu,
        prediction_kernel,
        derivation_kernel,
    )
}

criterion_group! {name = gpu_benches;
config = Criterion::default().measurement_time(Duration::from_secs(10)).sample_size(10);
targets=run_benches}
criterion_main!(gpu_benches);
