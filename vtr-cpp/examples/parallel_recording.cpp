#include "vtr_reader.h"
#include "vtr_writer.h"

#include <array>
#include <exception>
#include <iostream>
#include <thread>

struct Request {
    std::uint64_t address;
    std::string operation;
    std::vector<std::uint32_t> words;
};

template <> struct vtr::Schema<Request> {
    static constexpr std::string_view name = "Request";
    static constexpr auto fields = std::tuple{
        vtr::field("address", &Request::address),
        vtr::field("operation", &Request::operation),
        vtr::field("words", &Request::words),
    };
};

int main(int argc, char** argv) {
    namespace vt = vtr;
    try {
        auto path = argc > 1 ? argv[1] : "requests.vtr";
        vt::Writer trace(path, {.encoding = {.workers = 2}});
        auto requests = trace.transactions<Request>({.name = "requests"});
        constexpr unsigned workers = 4;
        std::vector<vt::ProducerId> producers;
        for (unsigned i = 0; i < workers; ++i) {
            producers.push_back(trace.add_producer());
        }
        std::array<std::exception_ptr, workers> errors;
        std::vector<std::jthread> threads;
        for (unsigned worker = 0; worker < workers; ++worker) {
            threads.emplace_back([&, worker] {
                try {
                    trace.use_producer(producers[worker]);
                    Request request{0, "load", {1, 2, 3}};
                    for (std::uint64_t i = 0; i < 100; ++i) {
                        trace.advance_time(i * 2);
                        request.address = 0x1000 + worker * 0x1000 + i * 4;
                        auto tx = trace.begin(requests, request);
                        trace.advance_time(i * 2 + 1);
                        trace.end(tx, vt::Status::ok);
                    }
                    trace.finish_producer();
                } catch (...) {
                    errors[worker] = std::current_exception();
                    trace.release_producer();
                }
            });
        }
        for (auto& thread : threads) {
            thread.join();
        }
        for (const auto& error : errors) {
            if (error) {
                std::rethrow_exception(error);
            }
        }
        trace.close();
        auto stats = trace.stats();
        std::cout << stats.transport.completed << " consumed operations\n";

        std::optional<vt::Record> retained;
        {
            vt::Reader input(path);
            input.transactions({}, [&](const vt::Record& record) {
                retained = record;
                return vt::Flow::stop;
            });
        }
        // Retained records own their payload and schema after the reader closes.
        auto request = retained->payload().as<Request>();
        std::cout << request.operation << " at " << request.address << ", " << request.words.size()
                  << " words\n";
    } catch (const vt::Error& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
