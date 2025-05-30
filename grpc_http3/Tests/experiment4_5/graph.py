import os
import re
import matplotlib.pyplot as plt
import numpy as np

def extract_data(file_name):
    x_vals = []
    y_vals = []
    
    with open(file_name, 'r') as file:
        for line in file:
            match = re.match(r"Time elapsed: \((\d+\.\d+) ,(\d+\.\d+)\)", line)
            if match:
                x_vals.append(float(match.group(1)))
                y_vals.append(float(match.group(2))*1000)
    
    return x_vals, y_vals

def process_folder(folder_path):
    if folder_path == "":
        print("No folder path provided.")
        return [], [], []
    
    all_x_vals = []
    all_y_vals = []
    max_length = 0
    
    for file_name in os.listdir(folder_path):
        file_path = os.path.join(folder_path, file_name)
        if os.path.isfile(file_path):
            print(f"Processing file: {file_name}")
            x_vals, y_vals = extract_data(file_path)
            max_length = max(max_length, len(x_vals))
            all_x_vals.append(x_vals)
            all_y_vals.append(y_vals)
    
    x_mean = []
    y_mean = []
    y_std = []
    y_min = []
    y_max = []

    for i in range(max_length):
        x_vals_at_index = []
        y_vals_at_index = []
        
        for x_vals, y_vals in zip(all_x_vals, all_y_vals):
            if i < len(x_vals):
                x_vals_at_index.append(x_vals[i])
                y_vals_at_index.append(y_vals[i])
        
        if x_vals_at_index:
            x_mean.append(np.median(x_vals_at_index))
        if y_vals_at_index:
            y_mean.append(np.median(y_vals_at_index))
            y_std.append(np.std(y_vals_at_index))
            y_min.append(np.min(y_vals_at_index))
            y_max.append(np.max(y_vals_at_index))
    
    return x_mean, y_mean, y_std 
    
def plot_comparison(x_mean1, y_mean1, y_std1, x_mean2, y_mean2, y_std2, folder1_name, folder2_name, title):
    plt.figure(figsize=(10, 6))
    
    x_not_mean1 = []
    x_not_mean2 = []
    
    for i in range(len(y_mean1)):
        if i == 0:
            x_not_mean1.append(0)
        else:
            x_not_mean1.append(x_mean1[i-1] + 0.200)

    for i in range(len(y_mean2)):
        if i == 0:
            x_not_mean2.append(0)
        else:
            x_not_mean2.append(x_mean2[i-1] + 0.200)

    if len(x_mean1) != 0:
        plt.plot(x_mean1, y_mean1, marker='o', linestyle='-', color='b', label="MPQUIC-[A-P]-Improved")
        y1_lower = np.array(y_mean1) - np.array(y_std1)
        y1_upper = np.array(y_mean1) + np.array(y_std1)
        plt.fill_between(x_mean1, y_mean1, y1_upper, color='b', alpha=0.20)

    if len(x_mean2) != 0:
        plt.plot(x_mean2, y_mean2, marker='x', linestyle='--', color='r', label="TONIC-Improved")
        y2_lower = np.array(y_mean2) - np.array(y_std2)
        y2_upper = np.array(y_mean2) + np.array(y_std2)
        plt.fill_between(x_mean2, y_mean2, y2_upper, color='r', alpha=0.20)
    
    plt.axvline(x=8, color='black', linestyle='solid')
    plt.axvline(x=13, color='black', linestyle='solid')
    plt.title(title)
    plt.xlabel('Time (s)')
    plt.yticks(range(0, 401, 25))
    plt.ylabel('Delay of msg exchange (ms)')
    plt.grid(True)
    plt.legend()
    plt.savefig('compare0.pdf')
    # plt.show()

# Main execution
def main():
    folder_path1 = input("Enter the first folder path containing the files: ")
    folder_path2 = input("Enter the second folder path containing the files: ")
    title = input("Enter the title for the plot: ")

    x_mean1, y_mean1, y_std1 = process_folder(folder_path1)
    x_mean2, y_mean2, y_std2 = process_folder(folder_path2)

    plot_comparison(x_mean1, y_mean1, y_std1, x_mean2, y_mean2, y_std2,
                    os.path.basename(folder_path1), os.path.basename(folder_path2), title)

if __name__ == "__main__":
    main()
