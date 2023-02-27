use crate::receiver::frame::{
    handler_to_xbi_result, instruction_error_to_xbi_result, invert_destination_from_message,
};
use crate::receiver::{frame::handler_to_dispatch_info, Receiver as ReceiverExt};
use codec::Encode;
use frame_support::pallet_prelude::DispatchResultWithPostInfo;
use frame_system::{ensure_signed, Config};
use sp_runtime::{traits::UniqueSaturatedInto, Either};
use sp_std::marker::PhantomData;
use xp_channel::{
    queue::{QueueSignal, Queueable},
    traits::XbiInstructionHandler,
    ChannelProgressionEmitter, Message,
};
use xp_format::{XbiFormat, XbiMetadata, XbiResult};

/// This is an asynchronous queue backed frame receiver, which expects some queue handler to transport the messages back via the transport layer,
/// detaching the message handling part with the transport of the message.
pub struct Receiver<T, Emitter, Queue, InstructionHandler> {
    phantom: PhantomData<(T, Emitter, Queue, InstructionHandler)>,
}

impl<T, Emitter, Queue, InstructionHandler> ReceiverExt
    for Receiver<T, Emitter, Queue, InstructionHandler>
where
    T: Config,
    Emitter: ChannelProgressionEmitter,
    Queue: Queueable<(Message, QueueSignal)>,
    InstructionHandler: XbiInstructionHandler<T::Origin>,
{
    type Origin = T::Origin;
    type Outcome = DispatchResultWithPostInfo;

    /// Request should always run the instruction, and produce some info containing meters for the execution
    fn handle_request(origin: &Self::Origin, msg: &mut XbiFormat) -> DispatchResultWithPostInfo {
        let _who = ensure_signed(origin.clone())?;
        let current_block: u32 = <frame_system::Pallet<T>>::block_number().unique_saturated_into();

        // progress to delivered
        msg.metadata.timesheet.progress(current_block);

        Emitter::emit_received(Either::Left(msg));

        invert_destination_from_message(&mut msg.metadata);

        let xbi_id = msg.metadata.id::<T::Hashing>();

        let instruction_result = InstructionHandler::handle(origin, msg);

        let xbi_result = match &instruction_result {
            Ok(info) => handler_to_xbi_result::<Emitter>(&xbi_id.encode(), info, msg),
            Err(e) => instruction_error_to_xbi_result(&xbi_id.encode(), e),
        };

        // progress to executed
        msg.metadata.timesheet.progress(current_block);

        Emitter::emit_request_handled(
            &xbi_result,
            &msg.metadata,
            &match &instruction_result {
                Ok(info) => Some(info.weight),
                Err(e) => e.post_info.actual_weight,
            }
            .unwrap_or_default(),
        );

        Queue::push((
            Message::Response(xbi_result, msg.metadata.clone()),
            QueueSignal::ResponseReceived,
        ));

        handler_to_dispatch_info(instruction_result)
    }

    /// Response should delegate to the queue handler who would know about how to handle the message
    fn handle_response(
        origin: &Self::Origin,
        msg: &XbiResult,
        metadata: &XbiMetadata,
    ) -> DispatchResultWithPostInfo {
        let _who = ensure_signed(origin.clone())?;
        Emitter::emit_received(Either::Right(msg));

        Queue::push((
            Message::Response(msg.clone(), metadata.clone()),
            QueueSignal::ResponseReceived,
        ));

        // TODO: add the cost of handling this response here
        Ok(Default::default())
    }
}