// TODO: add comments
macro_rules! stage_test_suite {
    ($runner:ident) => {
        /// Check that the execution is short-circuited if the database is empty.
        #[tokio::test]
        async fn execute_empty_db() {
            let runner = $runner::default();
            let input = crate::stage::ExecInput::default();
            let result = runner.execute(input).await.unwrap();
            assert_matches!(
                result,
                Err(crate::error::StageError::DatabaseIntegrity(_))
            );
            assert!(runner.validate_execution(input, result.ok()).is_ok(), "execution validation");
        }

        /// Check that the execution is short-circuited if the target was already reached.
        #[tokio::test]
        async fn execute_already_reached_target() {
            let stage_progress = 1000;
            let mut runner = $runner::default();
            let input = crate::stage::ExecInput {
                previous_stage: Some((crate::test_utils::PREV_STAGE_ID, stage_progress)),
                stage_progress: Some(stage_progress),
            };
            let seed = runner.seed_execution(input).expect("failed to seed");
            let rx = runner.execute(input);
            runner.after_execution(seed).await.expect("failed to run after execution hook");
            let result = rx.await.unwrap();
            assert_matches!(
                result,
                Ok(ExecOutput { done, reached_tip, stage_progress })
                    if done && reached_tip && stage_progress == stage_progress
            );
            assert!(runner.validate_execution(input, result.ok()).is_ok(), "execution validation");
        }

        #[tokio::test]
        async fn execute() {
            let (previous_stage, stage_progress) = (1000, 100);
            let mut runner = $runner::default();
            let input = crate::stage::ExecInput {
                previous_stage: Some((crate::test_utils::PREV_STAGE_ID, previous_stage)),
                stage_progress: Some(stage_progress),
            };
            let seed = runner.seed_execution(input).expect("failed to seed");
            let rx = runner.execute(input);
            runner.after_execution(seed).await.expect("failed to run after execution hook");
            let result = rx.await.unwrap();
            println!("RESULT >>> {:?}", result.unwrap_err().to_string());
            // assert_matches!(
            //     result,
            //     Ok(ExecOutput { done, reached_tip, stage_progress })
            //         if done && reached_tip && stage_progress == previous_stage
            // );
            // assert!(runner.validate_execution(input, result.ok()).is_ok(), "execution validation");
        }

        #[tokio::test]
        // Check that unwind does not panic on empty database.
        async fn unwind_empty_db() {
            let runner = $runner::default();
            let input = crate::stage::UnwindInput::default();
            let rx = runner.unwind(input);
            assert_matches!(
                rx.await.unwrap(),
                Ok(UnwindOutput { stage_progress }) if stage_progress == input.unwind_to
            );
            assert!(runner.validate_unwind(input).is_ok(), "unwind validation");
        }

        #[tokio::test]
        async fn unwind() {
            let (previous_stage, stage_progress) = (1000, 100);
            let mut runner = $runner::default();

            // Run execute
            let execute_input = crate::stage::ExecInput {
                previous_stage: Some((crate::test_utils::PREV_STAGE_ID, previous_stage)),
                stage_progress: Some(stage_progress),
            };
            let seed = runner.seed_execution(execute_input).expect("failed to seed");
            let rx = runner.execute(execute_input);
            runner.after_execution(seed).await.expect("failed to run after execution hook");
            let result = rx.await.unwrap();
            assert_matches!(
                result,
                Ok(ExecOutput { done, reached_tip, stage_progress })
                    if done && reached_tip && stage_progress == previous_stage
            );
            assert!(runner.validate_execution(execute_input, result.ok()).is_ok(), "execution validation");

            let unwind_input = crate::stage::UnwindInput {
                unwind_to: stage_progress, stage_progress, bad_block: None,
            };
            let rx = runner.unwind(unwind_input);
            assert_matches!(
                rx.await.unwrap(),
                Ok(UnwindOutput { stage_progress }) if stage_progress == unwind_input.unwind_to
            );
            assert!(runner.validate_unwind(unwind_input).is_ok(), "unwind validation");
        }
    };
}

pub(crate) use stage_test_suite;
