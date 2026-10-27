// Static callsite metadata and typed arguments support deferred log formatting.
#include "logging.h"
#include "vtr_logging.h"

#include <iostream>
#include <string>

int main(int argc, char** argv) {
    try {
        const auto path = argc > 1 ? argv[1] : "logging.vtr";
        vtr::Writer trace(path);
        logging::Logger logger(trace);

        LOG_INFO(logger, "Starting worker {}", "cpu0");
        for (std::uint32_t request = 0; request < 3; ++request) {
            trace.advance_time(request + 1);
            std::string operation = "load";
            LOG_INFO(logger, "request={} op={} address={:#x}", request, operation,
                     std::uint64_t{0x8000} + request * 4);
            // Borrowed text may be reused as soon as the logging call returns.
            operation.assign("overwritten");
        }
        LOG_WARNING(logger, "queue depth={} limit={}", std::uint32_t{8}, std::uint32_t{8});
        LOG_INFO(logger, "Worker stopped");
        trace.close();

        vtr::Reader input(path);
        input.records({.kind = vtr::RecordKind::log}, [&](const vtr::Record& record) {
            std::cout << vtr::logging::format_log(input, record) << '\n';
            return vtr::Flow::next;
        });
        (void)input.validate();
    } catch (const vtr::Error& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
